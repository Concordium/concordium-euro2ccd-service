mod config;
mod database;
mod helpers;
mod node;
mod prometheus;
mod secretsmanager;
mod sources;

use anyhow::{ensure, Context};
use clap::AppSettings;
use concordium_rust_sdk::endpoints;
use config::MAX_TIME_CHECK_SUBMISSION;
use helpers::{compute_median, convert_big_fraction_to_exchange_rate, get_signer, relative_change};
use node::{check_update_status, get_block_summary, get_node_client, send_update};
use num_rational::BigRational;
use reqwest::Url;
use secretsmanager::{get_governance_from_aws, get_governance_from_file};
use sources::{pull_exchange_rate, Source};
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
        long = "pull-interval",
        help = "How often to pull new exchange rate from each source. (In seconds)",
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
        long = "max-rates-saved",
        help = "Determines the size of the history of rates from the exchange",
        env = "EUR2CCD_SERVICE_MAX_RATES_SAVED",
        default_value = "60"
    )]
    max_rates_saved: usize,
    #[structopt(
        long = "test-source",
        help = "If set to true, pulls exchange rate from each of the given locations (see \
                local_exchange subproject)  (FOR TESTING)",
        env = "EUR2CCD_SERVICE_TEST_SOURCE",
        use_delimiter = true,
        group = "testing"
    )]
    test_sources: Vec<Url>,
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
    #[structopt(
        long = "database-url",
        help = "MySQL Connection url for a database, where every reading and update is inserted",
        env = "EUR2CCD_SERVICE_DATABASE_URL"
    )]
    database_url: Option<String>,
    #[structopt(
        long = "coin-gecko",
        help = "If this flag is enabled, Coin Gecko is added to the list of sources",
        env = "EUR2CCD_SERVICE_COIN_GECKO"
    )]
    coin_gecko: bool,
    #[structopt(
        long = "coin-market-cap",
        help = "This option expects an API key for Coin Market Cap, and if given Coin Market Cap \
                is added to the list of sources.",
        env = "EUR2CCD_SERVICE_COIN_MARKET_CAP"
    )]
    coin_market_cap: Option<String>,
    #[structopt(
        long = "live-coin-watch",
        help = "This option expects an API key for Live Coin Watch, and if given Live Coin Watch \
                is added to the list of sources.",
        env = "EUR2CCD_SERVICE_LIVE_COIN_WATCH"
    )]
    live_coin_watch: Option<String>,
    #[structopt(
        long = "bitfinex",
        help = "If this flag is enabled, BitFinex is added to the list of sources",
        env = "EUR2CCD_SERVICE_BITFINEX"
    )]
    bitfinex: bool,
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
/// last [config::MAX_RATES_SAVED] queries.
/// In the main task the service attempts to update the exchange rate every
/// `update-interval` seconds. It does this by looking at the last
/// [config::MAX_RATES_SAVED] exchange rates and deriving the update
/// exchange rate from those, by ignoring outliers, etc. This exchange rate is
/// then submitted to the chain, and queried until the transaction is finalized

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let app: App = {
        let app = App::clap().global_setting(AppSettings::ColoredHelp);
        let matches = app.get_matches();
        App::from_clap(&matches)
    };
    let max_rates_saved = app.max_rates_saved;
    let pull_interval = app.pull_interval;

    // Setup
    // (Stop if error occurs)
    let mut log_builder = env_logger::Builder::new();

    log_builder.filter_module(module_path!(), app.log_level);
    log_builder.init();

    log::debug!("Updating every {} seconds)", app.update_interval);
    log::debug!(
        "Warnings will be triggered when updates increase by {}% or decrease by {}%",
        app.warning_increase_threshold,
        app.warning_decrease_threshold
    );
    log::debug!(
        "Protected mode will be engaged when updates increase by {}% or decrease by {}%",
        app.halt_increase_threshold,
        app.halt_decrease_threshold
    );
    log::debug!(
        "Pulling rates every {} seconds. (Max {} rates are saved at a time)",
        pull_interval,
        max_rates_saved
    );

    ensure!(!app.endpoint.is_empty(), "At least one node must be provided.");
    ensure!(
        app.halt_increase_threshold > app.warning_increase_threshold,
        "Warning threshold must be lower than halt threshold (increase)"
    );
    ensure!(
        !(1..=100).contains(&app.halt_decrease_threshold),
        "Halt threshold (decrease) outside of allowed range (1-100): {} ",
        app.halt_decrease_threshold
    );
    ensure!(
        app.halt_decrease_threshold > app.warning_decrease_threshold,
        "Warning threshold must be lower than halt threshold (decrease)"
    );

    let million = BigRational::from_integer(1000000.into()); // 1000000 microCCD/CCD

    let (mut main_database_conn, connection_pool) = {
        if let Some(url) = app.database_url {
            let pool = database::establish_connection_pool(&url)?;
            let mut main_conn = pool.get_conn()?;
            database::create_tables(&mut main_conn)?;
            (Some(main_conn), Some(pool))
        } else {
            log::warn!(
                "No database url provided, service will not save to read and updated rates!"
            );
            (None, None)
        }
    };

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

    // Vector that stores the rate history for each source. Each history is a queue
    // in a mutex.
    let mut rate_histories: Vec<Arc<Mutex<VecDeque<BigRational>>>> = Vec::new();

    let mut add_source = |source: Source| -> anyhow::Result<()> {
        let rates_mutex = Arc::new(Mutex::new(VecDeque::with_capacity(max_rates_saved)));
        rate_histories.push(rates_mutex.clone());
        // Create a connection for this reader thread, if a database url was provided:
        let reader_conn = match connection_pool.clone() {
            Some(ref p) => Some(p.get_conn()?),
            None => None,
        };

        tokio::spawn(pull_exchange_rate(
            stats.clone(),
            source,
            rates_mutex,
            pull_interval,
            max_rates_saved,
            reader_conn,
        ));
        Ok(())
    };

    if app.coin_gecko {
        log::info!("Using \"Coin Gecko\" as a source");
        add_source(Source::CoinGecko)?
    }

    if app.bitfinex {
        log::info!("Using \"BitFinex\" as a source");
        add_source(Source::Bitfinex)?
    }

    if let Some(api_key) = app.coin_market_cap {
        log::info!("Using \"Coin Market Cap\" as a source");
        add_source(Source::CoinMarketCap(api_key))?
    }

    if let Some(api_key) = app.live_coin_watch {
        log::info!("Using \"Live Coin Watch\" as a source");
        add_source(Source::LiveCoinWatch(api_key))?
    }

    for (i, url) in app.test_sources.into_iter().enumerate() {
        log::info!("Using test source: {}, as test{}", url, i);
        add_source(Source::Test {
            url,
            label: format!("test{}", i),
        })?
    }

    ensure!(!rate_histories.is_empty(), "At least one source must be chosen.");

    let forced_dry_run = is_dry_run_forced();
    if forced_dry_run {
        log::warn!("Entering forced dry run. (No updates will performed)");
    }

    let mut signer = if app.dry_run || forced_dry_run {
        log::debug!("Running dry run!");
        stats.set_protected();
        None
    } else {
        log::debug!("Running wet run!");
        let secret_keys = if app.local_keys.is_empty() {
            ensure!(
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
            // For each source, we compute the median of their history:
            let rate_medians = rate_histories
                .iter()
                .map(|rates_mutex| {
                    let rates_lock = rates_mutex.lock().unwrap();
                    compute_median(&*rates_lock)
                })
                .collect::<Option<VecDeque<_>>>();
            // Then we determine the median of the medians:
            match rate_medians.and_then(|rm| compute_median(&rm)) {
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
                        if let Some(ref mut database_conn) = main_database_conn {
                            if let Err(e) = database::write_update_rate(database_conn, new_rate) {
                                stats.increment_failed_database_updates();
                                log::error!(
                                    "Unable to INSERT new update: {:?}, due to: {}",
                                    new_rate,
                                    e
                                )
                            };
                        }
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
