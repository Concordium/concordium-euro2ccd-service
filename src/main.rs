mod exchanges;
mod helpers;
mod prometheus;
mod secretsmanager;
mod node;

use anyhow::{anyhow, Result};
use clap::AppSettings;
use concordium_rust_sdk::{
    endpoints,
    types::{UpdateSequenceNumber},
};
use exchanges::{pull_exchange_rate, Exchange};
use secretsmanager::{get_governance_from_aws, get_governance_from_file};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use structopt::{clap::ArgGroup, StructOpt};
use tokio::time::{interval, sleep, timeout, Duration};
use node::{check_update_status, get_block_summary, send_update};
use helpers::{compute_average, convert_big_fraction_to_exchange_rate, get_signer};
use num_rational::BigRational;

const MAX_TIME_CHECK_SUBMISSION: u64 = 120; // seconds

#[derive(StructOpt)]
#[structopt(group = ArgGroup::with_name("testing").requires("test").multiple(true))]
struct App {
    #[structopt(
        long = "node",
        help = "GRPC interface of the node(s).",
        default_value = "http://localhost:10000",
        use_delimiter = true,
        env = "EURO2CCD_SERVICE_NODE"
    )]
    endpoint:        endpoints::Endpoint,
    #[structopt(
        long = "rpc-token",
        help = "GRPC interface access token for accessing all the nodes.",
        default_value = "rpcadmin",
        env = "EURO2CCD_SERVICE_RPC_TOKEN"
    )]
    token:           String,
    #[structopt(
        long = "secret-name",
        help = "Secret name on AWS.",
        env = "EURO2CCD_SERVICE_SECRET_NAME",
        default_value = "secret-dummy",
        required_unless = "local-keys",
        conflicts_with = "local-keys"
    )]
    secret_name:     String,
    #[structopt(
        long = "update-interval",
        help = "How often to perform the update. (In seconds)",
        env = "EURO2CCD_SERVICE_UPDATE_INTERVAL",
        default_value = "60"
    )]
    update_interval: u64,
    #[structopt(
        long = "log-level",
        default_value = "info",
        help = "Maximum log level.",
        env = "EURO2CCD_SERVICE_LOG_LEVEL"
    )]
    log_level:       log::LevelFilter,
    #[structopt(
        long = "max-change",
        help = "percentage max change allowed when updating exchange rate. i.e. 1-99",
        env = "EURO2CCD_SERVICE_MAX_CHANGE"
    )]
    max_change:      u8,
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
    test:            bool,
    #[structopt(
        long = "local-exchange",
        help = "If set to true, pulls exchange rate from localhost:8111 (see local_exchange \
                subproject)  (FOR TESTING)",
        env = "EURO2CCD_SERVICE_LOCAL_EXCHANGE",
        group = "testing"
    )]
    local_exchange:  bool,
    #[structopt(
        long = "local-keys",
        help = "If given, the service uses local governance keys in specified file instead of \
                pulling them from aws. (FOR TESTING) ",
        env = "EURO2CCD_SERVICE_LOCAL_KEYS",
        group = "testing"
    )]
    local_keys:      Option<Vec<PathBuf>>,
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

    let max_change = app.max_change;
    if !(1..=99).contains(&max_change) {
        log::error!("Max change outside of allowed range (1-99): {} ", max_change);
        return Err(anyhow!("Error during startup"));
    }

    tokio::spawn(prometheus::initialize_prometheus(app.prometheus_port));
    // Short sleep to allow prometheus to start (TODO: Get signal from prometheus
    // thread instead)
    sleep(Duration::from_secs(5)).await;

    let summary = get_block_summary(node_client.clone()).await?;
    let mut seq_number = summary.updates.update_queues.micro_gtu_per_euro.next_sequence_number;
    let initial_rate = summary.updates.chain_parameters.micro_gtu_per_euro;
    log::info!("Loaded initial block summary, current exchange rate: {:#?}", initial_rate);

    let exchange = match app.local_exchange {
        true => Exchange::Local,
        false => Exchange::Bitfinex,
    };

    let million = BigRational::from_integer(1000000.into()); // 1000000  microCCD/CCD

    let rates_mutex = Arc::new(Mutex::new(VecDeque::with_capacity(30)));

    // Add the current exchange rate to the list of rates.
    let mut initial_rate_big = BigRational::new(initial_rate.numerator.into(), initial_rate.denominator.into());
    initial_rate_big /= &million; // 1/1000000  CCD/microCCD
    rates_mutex.lock().unwrap().push_back(initial_rate_big);
    tokio::spawn(pull_exchange_rate(exchange, rates_mutex.clone()));

    let secret_keys = match app.local_keys {
        Some(path) => get_governance_from_file(path),
        None => get_governance_from_aws(&app.secret_name).await,
    }?;

    let signer = get_signer(secret_keys, &summary).await?;
    log::info!("keys loaded");

    let mut interval = interval(Duration::from_secs(app.update_interval));

    let conversion_threshold = BigRational::new(1.into(), 1000000000000u64.into());

    // Main Loop

    log::info!("Entering main loop");
    loop {
        log::debug!("Starting new main loop cycle: waiting for interval");
        interval.tick().await;

        log::debug!("waiting for lock");
        let rates = rates_mutex.lock().unwrap();
        log::debug!("got lock");
        let rate = match compute_average(rates.clone()) {
            Some(r) => r,
            None => {
                log::error!("Unable to compute average to rational");
                continue
            }
        };
        drop(rates);
        log::debug!("computed average: {:#?}, dropped lock", rate);

        // Convert the rate into an exchange_rate (i.e. convert the bigints to u64's). Also multiplies with 1000000 microCCD/CCD
        let new_rate = convert_big_fraction_to_exchange_rate(rate * &million, conversion_threshold.clone());
        log::debug!("converted new_rate: {:#?}", new_rate);

        let (submission_id, new_seq_number) =
            send_update(seq_number, &signer, new_rate, node_client.clone()).await;
        // new_seq_number is the sequence number, which was used to successfully send
        // the update.
        seq_number = UpdateSequenceNumber {
            number: new_seq_number.number + 1,
        };
        log::info!("sent update with submission id: {}", submission_id);

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
