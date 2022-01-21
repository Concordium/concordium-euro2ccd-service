use anyhow::{anyhow, Context, Result};
use clap::AppSettings;
use fraction::Fraction;
use serde_json::json;
use std::path::PathBuf;
use structopt::StructOpt;

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

const MAX_TIME_CHECK_SUBMISSION: u64 = 60; // seconds
const CHECK_SUBMISSION_STATUS_INTERVAL: u64 = 3; // seconds
const RETRY_SUBMISSION_INTERVAL: u64 = 10; // seconds
const RETRY_BITFINEX_INTERVAL: u64 = 10; // seconds
const BITFINEX_URL: &str = "https://api-pub.bitfinex.com/v2/calc/fx";
const UPDATE_EXPIRY_OFFSET: u64 = 300; // seconds
const UPDATE_EFFECTIVE_TIME_OFFSET: u64 = 301; // seconds

#[derive(StructOpt)]
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
    #[structopt(long = "key", help = "Path to update keys to use.", env = "EURO2CCD_SERVICE_KEYS")]
    governance_keys: Vec<PathBuf>,
    #[structopt(
        long = "update-interval",
        help = "How often to perform the update, in minutes.",
        env = "EURO2CCD_SERVICE_UPDATE_INTERVAL",
        default_value = "1"
    )]
    update_interval: u64,
    #[structopt(
        long = "log-level",
        default_value = "off",
        help = "Maximum log level.",
        env = "EURO2CCD_SERVICE_LOG_LEVEL"
    )]
    log_level:       log::LevelFilter,
    #[structopt(long = "max-change", help = "percentage max change allowed when updating exchange rate.", env = "EURO2CCD_SERVICE_MAX_CHANGE")]
    max_change: f64,
}

fn convert_fraction_to_exchange_rate(frac: Fraction) -> Result<ExchangeRate> {
    let numerator = match frac.numer() {
        Some(e) => e,
        None => return Err(anyhow!("unable to get numerator")),
    };
    let denominator = match frac.denom() {
        Some(e) => e,
        None => return Err(anyhow!("unable to get denominator")),
    };
    Ok(ExchangeRate {
        numerator:   *numerator,
        denominator: *denominator,
    })
}

async fn request_exchange_rate_bitfinex(client: reqwest::Client) -> f64 {
    // TODO: replace ADA with CCD
    let params = json!({"ccy1": "EUR", "ccy2": "ADA"});

    let mut interval =
        tokio::time::interval(tokio::time::Duration::from_secs(RETRY_BITFINEX_INTERVAL));
    loop {
        interval.tick().await;

        let resp = match client.post(BITFINEX_URL).json(&params).send().await {
            Ok(o) => o,
            Err(e) => {
                log::warn!("Unable to retrieve from bitfinex: {:#?}", e);
                continue;
            }
        };

        if resp.status().is_success() {
            // Bitfinex api speficies that a succesful status means the response is a json
            // array with a single float number.
            match resp.json::<Vec<f64>>().await {
                Ok(v) => return v[0],
                Err(_) => {
                    log::error!("Unable to parse response from bitfinex as JSON (Breaking API)")
                }
            };
        } else {
            log::error!("Error response from bitfinex: {:?}", resp.status());
        };
    }
}

/**
 * Get the new MicroCCD/Euro exchange rate
 */
async fn pull_exchange_rate(client: reqwest::Client) -> Result<ExchangeRate> {
    let raw_rate = request_exchange_rate_bitfinex(client).await;
    log::debug!("Raw exchange rate CCD/EUR polled from bitfinex: {:#?}", raw_rate);
    let ccd_rate = Fraction::from(raw_rate);
    // We multiply with 1/1000000 MicroCCD/CCD
    let micro_per_ccd = Fraction::new(1u64, 1000000u64);
    let micro_ccd_rate = ccd_rate * micro_per_ccd;
    convert_fraction_to_exchange_rate(micro_ccd_rate)
}

fn ensure_exchange_rate_within_bounds(current_exchange_rate: ExchangeRate, new_exchange_rate: ExchangeRate, max_change: f64) -> bool {
    let current = Fraction::new(current_exchange_rate.numerator, current_exchange_rate.denominator);
    let new = Fraction::new(new_exchange_rate.numerator, new_exchange_rate.denominator);
    let max_increase = current *  Fraction::from(1f64 + max_change);
    let max_decrease = current *  Fraction::from(1f64 - max_change);
    log::debug!("Allowed update range is {}-{}.", max_decrease, max_increase);
    new > max_decrease && new < max_increase
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
    keys: Vec<PathBuf>,
    summary: &BlockSummary,
) -> Result<Vec<(UpdateKeysIndex, KeyPair)>> {
    let kps: Vec<KeyPair> = keys
        .iter()
        .map(|p| {
            serde_json::from_reader(std::fs::File::open(p).context("Could not open file.")?)
                .context("Could not read keys from file.")
        })
        .collect::<anyhow::Result<_>>()?;

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
    let effective_time = TransactionTime::from_seconds(
        chrono::offset::Utc::now().timestamp() as u64 + UPDATE_EFFECTIVE_TIME_OFFSET,
    );
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

    let node_client = endpoints::Client::connect(app.endpoint, app.token).await?;

    let mut log_builder = env_logger::Builder::from_env("TRANSACTION_LOGGER_LOG");
    // only log the current module (main).
    log_builder.filter_module(module_path!(), app.log_level);
    log_builder.init();

    // Setup (Relaxed error handling)
    let max_change = app.max_change;

    let summary = get_block_summary(node_client.clone()).await?;
    let mut seq_number = summary.updates.update_queues.micro_gtu_per_euro.next_sequence_number;
    let mut prev_rate = summary.updates.chain_parameters.micro_gtu_per_euro;
    log::info!("Loaded initial block summary, current exchange rate: {:#?}" , prev_rate);
    let signer = get_signer(app.governance_keys, &summary).await?;
    log::info!("keys loaded");
    let client = reqwest::Client::new();

    let mut interval =
        tokio::time::interval(tokio::time::Duration::from_secs(app.update_interval * 60));

    // Main Loop
    log::info!("Entering main loop");
    loop {
        log::debug!("Starting new main loop cycle: waiting for interval");
        interval.tick().await;
        log::debug!("Polling for exchange rate");
        let rate = match pull_exchange_rate(client.clone()).await {
            Ok(rate) => rate,
            Err(e) => {
                log::error!("Unable to determine the current exchange rate: {:#?}", e);
                continue;
            }
        };
        log::info!("New exchange rate polled: {:#?}", rate);

        if !ensure_exchange_rate_within_bounds(prev_rate, rate, max_change) {
            log::warn!("New exchange rate outside of bounds: {:#?}", rate);
            continue
        }

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

        log::info!(
            "Succesfully updated exchange rate to: {:#?}, with id {}",
            rate,
            submission_id
        );
    }
}
