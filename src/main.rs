mod certificate_resolver;
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
use helpers::{
    compute_median, convert_big_fraction_to_exchange_rate, get_signer, relative_difference,
};
use node::{check_update_status, get_block_summary, send_update};
use num_rational::BigRational;
use reqwest::Url;
use secretsmanager::{get_governance_from_aws, get_governance_from_file};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use structopt::StructOpt;
use tokio::time::{interval_at, timeout, Duration, Instant};
use num_traits::CheckedDiv;

#[derive(StructOpt, Debug)]
struct App {
    #[structopt(
        long = "node",
        help = "location of the GRPC interface of the node.",
        default_value = "http://localhost:10000",
        env = "EUR2CCD_SERVICE_NODE"
    )]
    endpoint: endpoints::Endpoint,
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
        use_delimiter = true
    )]
    secret_names: Vec<String>,
    #[structopt(
        long = "bitfinex-cert",
        help = "Location of the bitfinex certificate file.",
        env = "EUR2CCD_SERVICE_BITFINEX_CERTIFICATE",
        default_value = config::BITFINEX_CERTIFICATE_LOCATION,
        conflicts_with = "test",
    )]
    bitfinex_cert: PathBuf,
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
        help = "How often to update the exchange rate. (In seconds)",
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
        long = "conversion_threshold_denominator",
        help = "Denominator for fraction that determines how far exchange rate can deviate from \
                actual (bigint) value. (the numerator is 1)",
        env = "EUR2CCD_SERVICE_CONVERSION_THRESHOLD_DENOMINATOR",
        default_value = "1000000000000"
    )]
    conversion_threshold_denominator: u64,
    #[structopt(
        long = "log-level",
        default_value = "info",
        help = "Maximum log level.",
        env = "EUR2CCD_SERVICE_LOG_LEVEL"
    )]
    log_level: log::LevelFilter,
    #[structopt(
        long = "warning-threshold",
        default_value = "30",
        help = "Determines the threshold where an update triggers a warning (relative difference \
                in percentage)",
        env = "EUR2CCD_SERVICE_WARNING_THRESHOLD"
    )]
    warning_threshold: u8,
    #[structopt(
        long = "halt-threshold",
        default_value = "100",
        help = "Determines the threshold where an update triggers a halt (relative difference in \
                percentage)",
        env = "EUR2CCD_SERVICE_HALT_THRESHOLD"
    )]
    halt_threshold: u8,
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
        long = "test",
        help = "If set, allows using test parameters.",
        env = "EUR2CCD_SERVICE_TEST",
        group = "testing"
    )]
    test: bool,
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

    log::debug!("Starting with configuration {:#?}", app);

    let node_client = endpoints::Client::connect(app.endpoint, app.token)
        .await
        .context("Could not connect to the node.")?;

    if !(1..=99).contains(&app.warning_threshold) {
        log::error!(
            "Warning threshold outside of allowed range (1-99): {} ",
            app.warning_threshold
        );
        bail!("Error during startup");
    }
    if !(1..=100).contains(&app.halt_threshold) {
        log::error!("Halt threshold outside of allowed range (1-100): {} ", app.halt_threshold);
        bail!("Error during startup");
    }
    if app.halt_threshold <= app.warning_threshold {
        log::error!("Warning threshold must be lower than halt threshold");
        bail!("Error during startup");
    }

    let million = BigRational::from_integer(1000000.into()); // 1000000 microCCD/CCD

    let warning_threshold = BigRational::from_integer(app.warning_threshold.into());
    let halt_threshold = BigRational::from_integer(app.halt_threshold.into());

    let (registry, stats) =
        prometheus::initialize().await.context("Failed to start the prometheus server.")?;
    tokio::spawn(prometheus::serve_prometheus(registry, app.prometheus_port));
    log::debug!("Started prometheus");

    let summary = get_block_summary(node_client.clone()).await?;
    let mut seq_number = summary.updates.update_queues.micro_gtu_per_euro.next_sequence_number;
    let initial_rate = summary.updates.chain_parameters.micro_gtu_per_euro;
    let mut prev_rate = BigRational::new(initial_rate.numerator.into(), initial_rate.denominator.into()).checked_div(&million).expect("Unable to convert current exchange rate to microccd/euro");
    log::debug!(
        "Loaded initial block summary, current exchange rate: {}/{}",
        initial_rate.numerator,
        initial_rate.denominator
    );

    let exchange = match app.test_exchange {
        Some(url) => Exchange::Test(url),
        None => Exchange::Bitfinex(app.bitfinex_cert),
    };


    let rates_mutex = Arc::new(Mutex::new(VecDeque::with_capacity(app.max_rates_saved)));

    tokio::spawn(pull_exchange_rate(
        stats.clone(),
        exchange,
        rates_mutex.clone(),
        app.pull_interval,
        app.max_rates_saved,
    ));

    let signer = if app.dry_run {
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
    let conversion_threshold =
        BigRational::new(1.into(), app.conversion_threshold_denominator.into());

    // Main Loop
    // Log errors, and move on

    log::info!("Entering main loop");
    loop {
        log::debug!("Starting new main loop cycle: waiting for interval");
        interval.tick().await;

        let rate = {
            let rates_lock = rates_mutex.lock().unwrap();
            match compute_median(&*rates_lock) {
                Some(r) => r,
                None => {
                    log::error!("Unable to compute median for update");
                    continue;
                }
            }
        }; // drop lock
        log::debug!("Computed median: {}", rate);

        // TODO: SHouldn't this be comparing actual rates, not ones off by a million?
        let diff = relative_difference(&rate, &prev_rate);
        if diff > halt_threshold {
            log::error!(
                "New update violates halt threshold, changing from {} to {} has ~{} % relative \
                 difference",
                prev_rate,
                rate,
                diff.round()
            );
            bail!("Halt threshold violated");
        } else if diff > warning_threshold {
            log::warn!(
                "New update violates warning threshold, changing from {} to {} has ~{} % relative \
                 difference",
                prev_rate,
                rate,
                diff.round()
            );
        }

        // Convert the rate into an ExchangeRate (i.e. convert the bigints to u64's).
        // Also multiplies with 1000000 microCCD/CCD
        let new_rate =
            convert_big_fraction_to_exchange_rate(&rate * &million, &conversion_threshold);
        log::debug!("Converted new_rate: {:#?}", new_rate);

        if let Some(signer) = signer.as_ref() {
            let (submission_id, new_seq_number) =
                send_update(&stats, seq_number, &signer, new_rate, node_client.clone()).await;
            log::info!("Sent update with submission id: {}", submission_id);

            match timeout(
                Duration::from_secs(MAX_TIME_CHECK_SUBMISSION),
                check_update_status(submission_id, node_client.clone()),
            )
            .await
            {
                Ok(submission_result) => {
                    // if we failed to submit, or to query, we retry with the same sequence number.
                    // if the previous transaction is already finalized this submission will fail,
                    // and send_update will retry with a new sequence number.
                    if let Err(e) = submission_result {
                        log::error!("Could not query submission status: {}.", e);
                    } else {
                        // new_seq_number is the sequence number, which was used to successfully
                        // send the update.
                        seq_number = new_seq_number.next();
                        prev_rate = rate;
                        log::info!(
                            "Succesfully updated exchange rate to: {:#?} microCCD/CCD, with id {}",
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
