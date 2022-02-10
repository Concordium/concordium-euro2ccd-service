mod config;
mod exchanges;
mod helpers;
mod node;
mod prometheus;
mod secretsmanager;

use anyhow::{bail, Context};
use clap::AppSettings;
use concordium_rust_sdk::endpoints;
use config::MAX_TIME_CHECK_SUBMISSION;
use exchanges::{pull_exchange_rate, Exchange};
use helpers::{compute_median, convert_big_fraction_to_exchange_rate, get_signer, relative_change};
use node::{check_update_status, get_block_summary, get_node_client, send_update};
use num_rational::BigRational;
use reqwest::Url;
use secretsmanager::{get_governance_from_aws, get_governance_from_file};
use std::{
    collections::VecDeque,
    fs::File,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use structopt::StructOpt;
use tokio::time::{interval_at, timeout, Duration, Instant};

#[derive(StructOpt, Debug)]
struct App {
    #[structopt(
        long = "node",
        help = "Comma separated location(s) of the GRPC interface of the node(s).",
        default_value = "http://localhost:10000",
        use_delimiter = true,
        env = "EUR2CCD_SERVICE_NODE"
    )]
    endpoint: Vec<endpoints::Endpoint>,
    #[structopt(
        long = "rpc-token",
        help = "GRPC interface access token for accessing the node.",
        default_value = "rpcadmin",
        env = "EUR2CCD_SERVICE_RPC_TOKEN"
    )]
    token: String,
    #[structopt(
        long = "secret-names",
        help = "Secret names on AWS to get govenance keys from.",
        env = "EUR2CCD_SERVICE_SECRET_NAMES",
        conflicts_with = "local-keys",
        use_delimiter = true
    )]
    secret_names: Vec<String>,
    #[structopt(
        long = "aws-region",
        help = "Which AWS region to get the keys from.",
        env = "EUR2CCD_SERVICE_AWS_REGION",
        default_value = config::AWS_REGION,
        conflicts_with = "local-keys",
    )]
    region: String,
    #[structopt(
        long = "update-interval",
        help = "How often to update the exchange rate on chain. (In seconds)",
        env = "EUR2CCD_SERVICE_UPDATE_INTERVAL",
        default_value = "1800"
    )]
    update_interval: u32,
    #[structopt(
        long = "pull-exchange-interval",
        help = "How often to pull new exchange rate from exchange. (In seconds)",
        env = "EUR2CCD_SERVICE_PULL_INTERVAL",
        default_value = "60"
    )]
    pull_interval: u32,
    #[structopt(
        long = "log-level",
        default_value = "info",
        help = "Maximum log level.",
        env = "EUR2CCD_SERVICE_LOG_LEVEL"
    )]
    log_level: log::LevelFilter,
    #[structopt(
        long = "warning-increase-threshold",
        default_value = "30",
        help = "Determines the threshold where an update increasing the exchange rate triggers a \
                warning (specified in percentage)",
        env = "EUR2CCD_SERVICE_WARNING_INCREASE_THRESHOLD"
    )]
    warning_increase_threshold: u16,
    #[structopt(
        long = "halt-increase-threshold",
        default_value = "100",
        help = "Determines the threshold where an update increasing the exchange rate triggers a \
                halt (specified in percentage)",
        env = "EUR2CCD_SERVICE_HALT_INCREASE_THRESHOLD"
    )]
    halt_increase_threshold: u16,
    #[structopt(
        long = "warning-decrease-threshold",
        default_value = "15",
        help = "Determines the threshold where an update decreasing the exchange rate triggers a \
                warning (specified in percentage)",
        env = "EUR2CCD_SERVICE_WARNING_DECREASE_THRESHOLD"
    )]
    warning_decrease_threshold: u8,
    #[structopt(
        long = "halt-decrease-threshold",
        default_value = "50",
        help = "Determines the threshold where an update decreasing the exchange rate triggers a \
                halt (specified in percentage)",
        env = "EUR2CCD_SERVICE_HALT_DECREASE_THRESHOLD"
    )]
    halt_decrease_threshold: u8,
    #[structopt(
        long = "prometheus-port",
        default_value = "8112",
        help = "Port where prometheus client will serve metrics",
        env = "EUR2CCD_SERVICE_PROMETHEUS_PORT"
    )]
    prometheus_port: u16,
    #[structopt(
        long = "max_rates_saved",
        help = "Determines the size of the history of rates from the exchange",
        env = "EUR2CCD_SERVICE_MAXIMUM_RATES_SAVED",
        default_value = "60"
    )]
    max_rates_saved: usize,
    #[structopt(
        long = "test-exchange",
        help = "If set to true, pulls exchange rate from the given location (see local_exchange \
                subproject)  (FOR TESTING)",
        env = "EUR2CCD_SERVICE_TEST_EXCHANGE",
        group = "testing"
    )]
    test_exchange: Option<Url>,
    #[structopt(
        long = "local-keys",
        help = "If given, the service uses local governance keys in specified file instead of \
                pulling them from AWS.",
        env = "EUR2CCD_SERVICE_LOCAL_KEYS"
    )]
    local_keys: Vec<PathBuf>,
    #[structopt(
        long = "dry-run",
        help = "Do not perform updates, only log the update that would be performed.",
        env = "EUR2CCD_DRY_RUN"
    )]
    dry_run: bool,
}

/// Attempts to create a file, signalling that the service should be forced into
/// dry run mode.
fn force_dry_run() {
    if let Err(e) = File::create(config::FORCED_DRY_RUN_FILE) {
        log::error!("Failed creating file to force dry run: {}", e)
    }
}

/// Checks if the file, which force_dry_run creates, exists.
fn is_dry_run_forced() -> bool {
    std::path::Path::exists(std::path::Path::new(config::FORCED_DRY_RUN_FILE))
}

/// This main program loop.
/// The program is structured into two tasks. A background task is spawned that
/// continuously polls the exchange for the current exchange rate and saves the
/// last [config::MAXIMUM_RATES_SAVED] queries.
/// In the main task the service attempts to update the exchange rate every
/// `update-interval` seconds. It does this by looking at the last
/// [config::MAXIMUM_RATES_SAVED] exchange rates and deriving the update
/// exchange rate from those, by ignoring outliers, etc. This exchange rate is
/// then submitted to the chain, and queried until the transaction is finalized

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let app = {
        let app = App::clap().global_setting(AppSettings::ColoredHelp);
        let matches = app.get_matches();
        App::from_clap(&matches)
    };

    // Setup
    // (Stop if error occurs)
    let mut log_builder = env_logger::Builder::new();

    log_builder.filter_module(module_path!(), app.log_level);
    log_builder.init();

    log::debug!("Starting with configuration {:?}", app);

    anyhow::ensure!(!app.endpoint.is_empty(), "At least one node must be provided.");

    if app.halt_increase_threshold <= app.warning_increase_threshold {
        log::error!("Warning threshold must be lower than halt threshold (increase)");
        bail!("Error during startup");
    }
    if !(1..=100).contains(&app.halt_decrease_threshold) {
        log::error!(
            "Halt threshold (decrease) outside of allowed range (1-100): {} ",
            app.halt_decrease_threshold
        );
        bail!("Error during startup");
    }
    if app.halt_decrease_threshold <= app.warning_decrease_threshold {
        log::error!("Warning threshold must be lower than halt threshold (decrease)");
        bail!("Error during startup");
    }

    let million = BigRational::from_integer(1000000.into()); // 1000000 microCCD/CCD

    let warning_increase_threshold =
        BigRational::from_integer(app.warning_increase_threshold.into());
    let halt_increase_threshold = BigRational::from_integer(app.halt_increase_threshold.into());
    let warning_decrease_threshold =
        BigRational::from_integer(app.warning_decrease_threshold.into());
    let halt_decrease_threshold = BigRational::from_integer(app.halt_decrease_threshold.into());

    let (registry, stats) =
        prometheus::initialize().await.context("Failed to start the prometheus server.")?;
    tokio::spawn(prometheus::serve_prometheus(registry, app.prometheus_port));
    log::debug!("Started prometheus");

    let mut node_client = get_node_client(app.endpoint.clone(), &app.token).await?;
    let summary = get_block_summary(node_client.clone()).await?;
    let mut seq_number = summary.updates.update_queues.micro_gtu_per_euro.next_sequence_number;
    let initial_rate = summary.updates.chain_parameters.micro_gtu_per_euro;
    let mut prev_rate =
        BigRational::new(initial_rate.numerator.into(), initial_rate.denominator.into());
    log::debug!(
        "Loaded initial block summary, current exchange rate: {}/{}  (~ {}) microCCD/EUR",
        initial_rate.numerator,
        initial_rate.denominator,
        initial_rate.numerator as f64 / initial_rate.denominator as f64
    );

    let exchange = match app.test_exchange {
        Some(url) => Exchange::Test(url),
        None => Exchange::Bitfinex,
    };

    let rates_mutex = Arc::new(Mutex::new(VecDeque::with_capacity(app.max_rates_saved)));

    tokio::spawn(pull_exchange_rate(
        stats.clone(),
        exchange,
        rates_mutex.clone(),
        app.pull_interval,
        app.max_rates_saved,
    ));

    let forced_dry_run = is_dry_run_forced();
    if forced_dry_run {
        log::warn!("Entering forced dry run. (No updates will performed)");
    }

    let mut signer = if app.dry_run || forced_dry_run {
        stats.set_protected();
        None
    } else {
        let secret_keys = if app.local_keys.is_empty() {
            anyhow::ensure!(
                !app.secret_names.is_empty(),
                "If `dry-run` is not used then one of `secret-names` and `local-keys` must be \
                 provided."
            );
            get_governance_from_aws(app.region, app.secret_names).await
        } else {
            get_governance_from_file(&app.local_keys)
        }
        .context("Could not obtain keys.")?;
        Some(get_signer(secret_keys, &summary).context("Failed to obtain keys.")?)
    };

    let update_interval_duration = Duration::from_secs(app.update_interval.into());
    let mut interval =
        interval_at(Instant::now() + update_interval_duration, update_interval_duration);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    // Main Loop
    // Log errors, and move on

    log::info!("Entering main loop");
    'main: loop {
        log::debug!("Starting new main loop cycle: waiting for interval");
        interval.tick().await;

        let rate = {
            let rates_lock = rates_mutex.lock().unwrap();
            match compute_median(&*rates_lock) {
                Some(r) => r * &million, /* multiply with 1000000 microCCD/CCD to convert the */
                // unit to microCCD/Eur
                None => {
                    log::error!("Unable to compute median for update");
                    continue;
                }
            }
        }; // drop lock
        log::debug!("Computed median: {} microCCD/Eur", rate);

        // Calculates the relative change from the prev_rate, which should be the
        // current exchange rate on chain, and our proposed update:
        let diff = relative_change(&prev_rate, &rate);
        if rate > prev_rate {
            // Rate has increased
            if diff > halt_increase_threshold {
                log::error!(
                    "New update violates halt threshold, changing from {} to {} is an ~{} % \
                     increase (forcing dry run)",
                    prev_rate,
                    rate,
                    diff.round()
                );
                force_dry_run();
                signer = None;
                stats.set_protected();
                continue;
            } else if diff > warning_increase_threshold {
                log::warn!(
                    "New update violates warning threshold, changing from {} to {} has ~{} % \
                     increase",
                    prev_rate,
                    rate,
                    diff.round()
                );
                stats.increment_warning_threshold_violations();
            }
        } else {
            // Rate has decreased
            if diff > halt_decrease_threshold {
                log::error!(
                    "New update violates halt threshold, changing from {} to {} has ~{} % \
                     decrease (forcing dry run)",
                    prev_rate,
                    rate,
                    diff.round()
                );
                force_dry_run();
                signer = None;
                stats.set_protected();
                continue;
            } else if diff > warning_decrease_threshold {
                log::warn!(
                    "New update violates warning threshold, changing from {} to {} has ~{} % \
                     decrease",
                    prev_rate,
                    rate,
                    diff.round()
                );
                stats.increment_warning_threshold_violations();
            }
        }

        // Convert the rate into an ExchangeRate (i.e. convert the bigints to u64's).
        let new_rate = convert_big_fraction_to_exchange_rate(&rate);
        log::debug!("Converted new_rate: {:?}", new_rate);

        if let Some(signer) = signer.as_ref() {
            // Send the update to a node. This loop only terminates if the node accepts the
            // transaction or we can't connect to any node
            let (submission_id, new_seq_number) = {
                loop {
                    // Try to send the update
                    if let Some(result) =
                        send_update(&stats, seq_number, &signer, new_rate, node_client.clone())
                            .await
                    {
                        break result;
                    };
                    // We expect that connection/authentication problems would be the reason sending
                    // the update failed, so we try to connect to a new node.
                    // (Any other problem would be have to be fixed manually)
                    node_client = match get_node_client(app.endpoint.clone(), &app.token).await {
                        Ok(client) => client,
                        Err(e) => {
                            log::error!(
                                "Unable to connect to any node: {}, skipping this update",
                                e
                            );
                            continue 'main;
                        }
                    };
                }
            };
            log::info!("Sent update with submission id: {}", submission_id);

            match timeout(
                Duration::from_secs(MAX_TIME_CHECK_SUBMISSION),
                check_update_status(submission_id, node_client.clone()),
            )
            .await
            {
                Ok(submission_result) => {
                    // if we fail to confirm the transaction finalized, we retry with the same
                    // sequence number next update. if the previous transaction
                    // is already finalized this submission will fail,
                    // and send_update will retry with a new sequence number.
                    if let Err(e) = submission_result {
                        log::error!("Could not query submission status: {}.", e);
                    } else {
                        // new_seq_number is the sequence number, which was used to successfully
                        // send the update.
                        seq_number = new_seq_number.next();
                        stats.update_updated_rate(&rate);
                        prev_rate = rate;
                        log::info!(
                            "Succesfully updated exchange rate to: {:?} microCCD/CCD, with id {}",
                            new_rate,
                            submission_id
                        );
                    }
                }
                Err(e) => log::error!(
                    "Was unable to confirm update with id {} within allocated timeframe due to: {}",
                    submission_id,
                    e
                ),
            };
        } else {
            log::info!(
                "Dry run enabled, so skipping the update. New rate: {}/{}",
                new_rate.numerator,
                new_rate.denominator
            );
        }
    }
}
