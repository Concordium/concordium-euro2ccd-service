mod config;
mod exchanges;
mod helpers;
mod node;
mod prometheus;
mod secretsmanager;
mod certificate_resolver;

use anyhow::{anyhow, Result};
use clap::AppSettings;
use concordium_rust_sdk::{endpoints, types::UpdateSequenceNumber};
use config::MAX_TIME_CHECK_SUBMISSION;
use exchanges::{pull_exchange_rate, Exchange};
use helpers::{compute_average, convert_big_fraction_to_exchange_rate, get_signer};
use node::{check_update_status, get_block_summary, send_update};
use num_rational::BigRational;
use secretsmanager::{get_governance_from_aws, get_governance_from_file};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use structopt::{clap::ArgGroup, StructOpt};
use tokio::time::{interval_at, timeout, Duration, Instant};

#[derive(StructOpt)]
#[structopt(group = ArgGroup::with_name("testing").requires("test").multiple(true))]
struct App {
    #[structopt(
        long = "node",
        help = "location of the GRPC interface of the node.",
        default_value = "http://localhost:10000",
        use_delimiter = true,
        env = "EURO2CCD_SERVICE_NODE"
    )]
    endpoint: endpoints::Endpoint,
    #[structopt(
        long = "rpc-token",
        help = "GRPC interface access token for accessing the node.",
        default_value = "rpcadmin",
        env = "EURO2CCD_SERVICE_RPC_TOKEN"
    )]
    token: String,
    #[structopt(
        long = "secret-name",
        help = "Secret name on AWS.",
        env = "EURO2CCD_SERVICE_SECRET_NAME",
        default_value = "secret-dummy",
        required_unless = "local-keys",
        conflicts_with = "local-keys"
    )]
    secret_name: String,
    #[structopt(
        long = "update-interval",
        help = "How often to update the exchange rate. (In seconds)",
        env = "EURO2CCD_SERVICE_UPDATE_INTERVAL",
        default_value = "1800"
    )]
    update_interval: u64,
    #[structopt(
        long = "pull-exchange-interval",
        help = "How often to pull new exchange rate from exchange. (In seconds)",
        env = "EURO2CCD_SERVICE_PULL_INTERVAL",
        default_value = "60"
    )]
    pull_interval: u64,
    #[structopt(
        long = "conversion_threshold_denominator",
        help = "Denominator for fraction that determines how far exchange rate can deviate from \
                actual (bigint) value. (the numerator is 1)",
        env = "EURO2CCD_SERVICE_CONVERSION_TRESHOLD_DENOMINATOR",
        default_value = "1000000000000"
    )]
    conversion_threshold_denominator: u64,
    #[structopt(
        long = "log-level",
        default_value = "info",
        help = "Maximum log level.",
        env = "EURO2CCD_SERVICE_LOG_LEVEL"
    )]
    log_level: log::LevelFilter,
    #[structopt(
        long = "max-deviation",
        default_value = "30",
        help = "Percentage max change allowed when adding new readings to the history of exchange \
                rates. (1-99)",
        env = "EURO2CCD_SERVICE_MAX_DEVIATION"
    )]
    max_deviation: u8,
    #[structopt(
        long = "prometheus-port",
        default_value = "8112",
        help = "Port where prometheus client will serve metrics",
        env = "EURO2CCD_SERVICE_PROMETHEUS_PORT"
    )]
    prometheus_port: u16,
    #[structopt(
        long = "test",
        help = "If set, allows using test parameters.",
        env = "EURO2CCD_SERVICE_TEST",
        group = "testing"
    )]
    test: bool,
    #[structopt(
        long = "test-exchange",
        help = "If set to true, pulls exchange rate from the given location (see local_exchange \
                subproject)  (FOR TESTING)",
        env = "EURO2CCD_SERVICE_TEST_EXCHANGE",
        group = "testing"
    )]
    test_exchange: Option<String>,
    #[structopt(
        long = "local-keys",
        help = "If given, the service uses local governance keys in specified file instead of \
                pulling them from aws. (FOR TESTING) ",
        env = "EURO2CCD_SERVICE_LOCAL_KEYS",
        group = "testing"
    )]
    local_keys: Option<Vec<PathBuf>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let app = {
        let app = App::clap().global_setting(AppSettings::ColoredHelp);
        let matches = app.get_matches();
        App::from_clap(&matches)
    };

    // Setup
    // (Stop if error occurs)

    let mut log_builder = env_logger::Builder::from_env("TRANSACTION_LOGGER_LOG");
    // only log the current module (main).
    log_builder.filter_module(module_path!(), app.log_level);
    log_builder.init();

    let node_client = endpoints::Client::connect(app.endpoint, app.token).await?;

    if app.test {
        log::warn!("Running with test options enabled!");
    }

    let max_deviation = app.max_deviation;
    if !(1..=99).contains(&max_deviation) {
        log::error!("Max change outside of allowed range (1-99): {} ", max_deviation);
        return Err(anyhow!("Error during startup"));
    }

    prometheus::initialize_prometheus().await?;
    tokio::spawn(prometheus::serve_prometheus(app.prometheus_port));
    log::debug!("Started prometheus");

    let summary = get_block_summary(node_client.clone()).await?;
    let mut seq_number = summary.updates.update_queues.micro_gtu_per_euro.next_sequence_number;
    let initial_rate = summary.updates.chain_parameters.micro_gtu_per_euro;
    log::debug!("Loaded initial block summary, current exchange rate: {:#?}", initial_rate);

    let exchange = match app.test_exchange {
        Some(url) => Exchange::Test(url),
        None => Exchange::Bitfinex,
    };

    let million = BigRational::from_integer(1000000.into()); // 1000000 microCCD/CCD

    let rates_mutex = Arc::new(Mutex::new(VecDeque::with_capacity(30)));

    tokio::spawn(pull_exchange_rate(
        exchange,
        rates_mutex.clone(),
        max_deviation,
        app.pull_interval,
    ));

    let secret_keys = match app.local_keys {
        Some(path) => get_governance_from_file(path),
        None => get_governance_from_aws(&app.secret_name).await,
    }?;

    let signer = get_signer(secret_keys, &summary)?;
    log::debug!("keys loaded");

    let update_interval_duration = Duration::from_secs(app.update_interval);
    let mut interval =
        interval_at(Instant::now() + update_interval_duration, update_interval_duration);

    let conversion_threshold =
        BigRational::new(1.into(), app.conversion_threshold_denominator.into());

    // Main Loop
    // Log errors, and move on

    log::info!("Entering main loop");
    loop {
        log::debug!("Starting new main loop cycle: waiting for interval");
        interval.tick().await;

        let rates = rates_mutex.lock().unwrap();
        let rate = match compute_average(rates.clone()) {
            Some(r) => r,
            None => {
                log::error!("Unable to compute average, for update");
                continue;
            }
        };
        drop(rates);
        log::debug!("Computed average: {:#?}", rate);

        // Convert the rate into an ExchangeRate (i.e. convert the bigints to u64's).
        // Also multiplies with 1000000 microCCD/CCD
        let new_rate =
            convert_big_fraction_to_exchange_rate(rate * &million, conversion_threshold.clone());
        log::debug!("Converted new_rate: {:#?}", new_rate);

        let (submission_id, new_seq_number) =
            send_update(seq_number, &signer, new_rate, node_client.clone()).await;
        // new_seq_number is the sequence number, which was used to successfully send
        // the update.
        seq_number = UpdateSequenceNumber {
            number: new_seq_number.number + 1,
        };
        log::info!("Sent update with submission id: {}", submission_id);

        match timeout(
            Duration::from_secs(MAX_TIME_CHECK_SUBMISSION),
            check_update_status(submission_id, node_client.clone()),
        )
        .await
        {
            Ok(_) => log::info!(
                "Succesfully updated exchange rate to: {:#?} microCCD/CCD, with id {}",
                new_rate,
                submission_id
            ),
            Err(e) => log::error!(
                "Was unable to confirm update with id {} within allocated timeframe due to: {}",
                submission_id,
                e
            ),
        };
    }
}
