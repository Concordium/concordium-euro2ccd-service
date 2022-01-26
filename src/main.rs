mod exchanges;
mod helpers;
mod secretsmanager;
mod prometheus;

use anyhow::{anyhow, Context, Result};
use clap::AppSettings;
use concordium_rust_sdk::{
    constants::DEFAULT_NETWORK_ID,
    endpoints,
    types::{
        hashes,
        transactions::{update, BlockItem, Payload},
        BlockSummary, ExchangeRate, TransactionStatus, UpdateKeysIndex, UpdatePayload,
        UpdateSequenceNumber,
    },
};
use crypto_common::{
    base16_encode_string,
    types::{KeyPair, TransactionTime},
};
use exchanges::{pull_exchange_rate, Exchange};
use helpers::bound_exchange_rate_change;
use secretsmanager::{get_governance_from_aws, get_governance_from_file};
use std::path::PathBuf;
use structopt::{clap::ArgGroup, StructOpt};

const MAX_TIME_CHECK_SUBMISSION: u64 = 60; // seconds
const CHECK_SUBMISSION_STATUS_INTERVAL: u64 = 5; // seconds
const RETRY_SUBMISSION_INTERVAL: u64 = 10; // seconds
const UPDATE_EXPIRY_OFFSET: u64 = 100; // seconds

#[derive(StructOpt)]
#[structopt(group = ArgGroup::with_name("testing").requires("test").multiple(true))]
struct App {
    #[structopt(
        long = "node",
        help = "GRPC interface of the node(s).",
        default_value = "http://localhost:10000",
        use_delimiter = true,
        env = "EURO2CCD_SERVICE_NODES"
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
        required_unless = "test",
        conflicts_with = "local-keys"
    )]
    secret_name:     String,
    #[structopt(
        long = "update-interval",
        help = "How often to perform the update.",
        env = "EURO2CCD_SERVICE_UPDATE_INTERVAL",
        default_value = "60"
    )]
    update_interval: u64,
    #[structopt(
        long = "log-level",
        default_value = "off",
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
    prometheus_port:      u16,
    #[structopt(
        long = "test",
        help = "If set to true, allows using test parameters (FOR TESTING)",
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
        help = "If given, the service uses local governance keys instead of pulling them from \
                aws. (FOR TESTING) ",
        env = "EURO2CCD_SERVICE_LOCAL_KEYS",
        group = "testing"
    )]
    local_keys:      Option<Vec<PathBuf>>,
}

async fn get_block_summary(mut node_client: endpoints::Client) -> Result<BlockSummary> {
    let consensus_status = node_client
        .get_consensus_status()
        .await
        .context("Could not obtain status of consensus.")?;

    // Get the key indices, as well as the next sequence number from the last
    // finalized block.
    let summary: BlockSummary = node_client
        .get_block_summary(&consensus_status.last_finalized_block)
        .await
        .context("Could not obtain last finalized block")?;
    Ok(summary)
}

async fn get_signer(
    kps: Vec<KeyPair>,
    summary: &BlockSummary,
) -> Result<Vec<(UpdateKeysIndex, KeyPair)>> {
    let update_keys = &summary.updates.keys.level_2_keys.keys;
    let update_key_indices = &summary.updates.keys.level_2_keys.micro_gtu_per_euro;

    // find the key indices to sign with
    let mut signer = Vec::new();
    for kp in kps {
        if let Some(i) = update_keys.iter().position(|public| public.public == kp.public.into()) {
            let idx = UpdateKeysIndex {
                index: i as u16,
            };
            if update_key_indices.authorized_keys.contains(&idx) {
                signer.push((idx, kp))
            } else {
                anyhow::bail!(
                    "The given key {} is not registered for the CCD/Eur rate update.",
                    base16_encode_string(&kp.public)
                );
            }
        } else {
            anyhow::bail!(
                "The given key {} is not registered for any level 2 updates.",
                base16_encode_string(&kp.public)
            );
        }
    }
    Ok(signer)
}

fn construct_block_item(
    seq_number: UpdateSequenceNumber,
    signer: &[(UpdateKeysIndex, KeyPair)],
    exchange_rate: ExchangeRate,
) -> BlockItem<Payload> {
    let effective_time = 0.into();
    let timeout = TransactionTime::from_seconds(
        chrono::offset::Utc::now().timestamp() as u64 + UPDATE_EXPIRY_OFFSET,
    );
    let payload = UpdatePayload::MicroGTUPerEuro(exchange_rate);
    update::update(signer, seq_number, effective_time, timeout, payload).into()
}

async fn send_update(
    mut seq_number: UpdateSequenceNumber,
    signer: &[(UpdateKeysIndex, KeyPair)],
    exchange_rate: ExchangeRate,
    mut client: endpoints::Client,
) -> (hashes::TransactionHash, UpdateSequenceNumber) {
    let mut get_new_seq_number = false;

    let mut interval =
        tokio::time::interval(tokio::time::Duration::from_secs(RETRY_SUBMISSION_INTERVAL));
    loop {
        interval.tick().await;

        if get_new_seq_number {
            let new_summary = match get_block_summary(client.clone()).await {
                Ok(o) => o,
                Err(e) => {
                    log::warn!("Unable to pull new sequence number due to: {:#?}", e);
                    continue;
                }
            };
            seq_number = new_summary.updates.update_queues.micro_gtu_per_euro.next_sequence_number;
            get_new_seq_number = false;
        }

        let block_item = construct_block_item(seq_number, signer, exchange_rate);
        match client.send_transaction(DEFAULT_NETWORK_ID, &block_item).await {
            Ok(true) => return (block_item.hash(), seq_number),
            Ok(false) => {
                log::error!("Sending update was rejected, id: {:#?}.", block_item.hash());
                // We assume that the reason for rejection is an incorrect sequence number
                // (because it is the only one we can solve)
                get_new_seq_number = true;
            }
            Err(e) => log::warn!("Error occurred while sending update: {:#?}", e),
        }
    }
}

async fn check_update_status(
    submission_id: hashes::TransactionHash,
    mut client: endpoints::Client,
) -> Result<()> {
    let mut interval =
        tokio::time::interval(tokio::time::Duration::from_secs(CHECK_SUBMISSION_STATUS_INTERVAL));
    loop {
        interval.tick().await;
        match client
            .get_transaction_status(&submission_id)
            .await
            .context("Could not query submission status.")?
        {
            TransactionStatus::Finalized(blocks) => {
                log::info!(
                    "Submission is finalized in blocks {:?}",
                    blocks.keys().collect::<Vec<_>>()
                );
                break;
            }
            TransactionStatus::Committed(blocks) => {
                log::info!(
                    "Submission is committed to blocks {:?}",
                    blocks.keys().collect::<Vec<_>>()
                );
            }
            TransactionStatus::Received => log::debug!("Submission is received."),
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let app = {
        let app = App::clap()
        // .setting(AppSettings::ArgRequiredElseHelp)
            .global_setting(AppSettings::ColoredHelp);
        let matches = app.get_matches();
        App::from_clap(&matches)
    };

    // Setup
    // (Stop if error occurs)

    let node_client = endpoints::Client::connect(app.endpoint, app.token).await?;

    let mut log_builder = env_logger::Builder::from_env("TRANSACTION_LOGGER_LOG");
    // only log the current module (main).
    log_builder.filter_module(module_path!(), app.log_level);
    log_builder.init();

    if app.test {
        log::warn!("Running with test options enabled!");
    }

    let max_change = app.max_change;
    if !(1..=99).contains(&max_change) {
        log::error!("Max change outside of allowed range (1-99): {} ", max_change);
        return Err(anyhow!("Error during startup"));
    }

    tokio::spawn(prometheus::initialize_prometheus(app.prometheus_port));

    let summary = get_block_summary(node_client.clone()).await?;
    let mut seq_number = summary.updates.update_queues.micro_gtu_per_euro.next_sequence_number;
    let mut prev_rate = summary.updates.chain_parameters.micro_gtu_per_euro;
    log::info!("Loaded initial block summary, current exchange rate: {:#?}", prev_rate);

    let secret_keys = match app.local_keys {
        Some(path) => get_governance_from_file(path),
        None => get_governance_from_aws(&app.secret_name).await,
    }?;

    let signer = get_signer(secret_keys, &summary).await?;
    log::info!("keys loaded");

    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(app.update_interval));

    let exchange = match app.local_exchange {
        true => Exchange::Local,
        false => Exchange::Bitfinex,
    };

    // Main Loop

    log::info!("Entering main loop");
    loop {
        log::debug!("Starting new main loop cycle: waiting for interval");
        interval.tick().await;
        log::debug!("Polling for exchange rate");
        let rate = match pull_exchange_rate(exchange).await {
            Ok(rate) => rate,
            Err(e) => {
                log::error!("Unable to determine the current exchange rate: {:#?}", e);
                continue;
            }
        };
        log::info!("New exchange rate polled: {:#?}", rate);
        let rate = match bound_exchange_rate_change(prev_rate, rate, max_change) {
            Ok(rate) => rate,
            Err(e) => {
                log::error!("Bounding exchange rate failed: {:#?}", e);
                continue;
            }
        };

        let (submission_id, new_seq_number) =
            send_update(seq_number, &signer, rate, node_client.clone()).await;
        // new_seq_number should be the sequence number, which was used to send the
        // update.
        seq_number = UpdateSequenceNumber {
            number: new_seq_number.number + 1,
        };
        prev_rate = rate;
        log::info!("sent update with submission id: {}", submission_id);

        match tokio::time::timeout(
            tokio::time::Duration::from_secs(MAX_TIME_CHECK_SUBMISSION),
            check_update_status(submission_id, node_client.clone()),
        )
        .await
        {
            Ok(_) => (),
            Err(_) => {
                log::error!(
                    "Was unable to confirm update with id {} within allocated timeframe",
                    submission_id
                );
                continue;
            }
        };

        log::info!("Succesfully updated exchange rate to: {:#?}, with id {}", rate, submission_id);
    }
}
