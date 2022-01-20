use anyhow::{anyhow, Context, Result};
use clap::AppSettings;
use serde_json::json;
use structopt::StructOpt;
use std::path::PathBuf;
use fraction::Fraction;

use concordium_rust_sdk::{
    constants::DEFAULT_NETWORK_ID,
    endpoints,
    types::{
        hashes,
        transactions::{update, BlockItem, Payload},
        BlockSummary, ExchangeRate, TransactionStatus, UpdateKeysIndex, UpdatePayload,
    },
};
use crypto_common::{
    base16_encode_string,
    types::{KeyPair, TransactionTime},
};

#[derive(StructOpt)]
struct App {
    #[structopt(
        long = "node",
        help = "GRPC interface of the node(s).",
        default_value = "http://localhost:10500",
        use_delimiter = true,
        env = "EURO2CCD_SERVICE_NODES"
    )]
    endpoint: endpoints::Endpoint,
    #[structopt(
        long = "rpc-token",
        help = "GRPC interface access token for accessing all the nodes.",
        default_value = "rpcadmin",
        env = "EURO2CCD_SERVICE_RPC_TOKEN"
    )]
    token: String,
    #[structopt(long = "key", help = "Path to update keys to use.", env="EURO2CCD_SERVICE_KEYS")]
    governance_keys:     Vec<PathBuf>,
    #[structopt(
        long = "log-level",
        default_value = "off",
        help = "Maximum log level.",
        env = "EURO2CCD_SERVICE_LOG_LEVEL"
    )]
    log_level: log::LevelFilter,
}

/**
* Get the new MicroCCD/Euro exchange rate
*/
async fn pull_exchange_rate(client: reqwest::Client) -> Result<ExchangeRate> {
    let params = json!({"ccy1": "EUR", "ccy2": "ADA"});
    let req = client.post("https://api-pub.bitfinex.com/v2/calc/fx").json(&params);
    let resp = req.send().await?.json::<Vec<f64>>().await?;
    log::debug!("Raw exchange rate CCD/EUR polled: {:#?}", resp);
    let frac = Fraction::from(resp[0]);
    let numerator = match frac.numer() {
        Some(e) => Ok(e),
        None => Err(anyhow!("unable to get numerator"))
    }?;
    let denominator = match frac.denom() {
        Some(e) => Ok(e),
        None => Err(anyhow!("unable to get denominator"))
    }?;

    // We multiply with 1/1000000 MicroCCD/CCD
    Ok(ExchangeRate { numerator: *numerator, denominator: *denominator * 1000000 })
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

async fn get_signer(keys: Vec<PathBuf>, summary: &BlockSummary) -> Result<Vec<(UpdateKeysIndex, KeyPair)>> {
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
        if let Some(i) = update_keys
            .iter()
            .position(|public| public.public == kp.public.into())
        {
            let idx = UpdateKeysIndex { index: i as u16 };
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

async fn send_update(summary: BlockSummary, signer: Vec<(UpdateKeysIndex, KeyPair)>, exchange_rate: ExchangeRate,  mut client: endpoints::Client) -> Result<hashes::TransactionHash> {
    let seq_number = summary
        .updates
        .update_queues
        .micro_gtu_per_euro
        .next_sequence_number;
    let effective_time = TransactionTime::from_seconds(chrono::offset::Utc::now().timestamp() as u64 + 301); // 2.5min effectiveTime.
    let timeout =
        TransactionTime::from_seconds(chrono::offset::Utc::now().timestamp() as u64 + 300); // 5min expiry.
    let payload = UpdatePayload::MicroGTUPerEuro(exchange_rate);
    let block_item: BlockItem<Payload> = update::update(
        signer.as_slice(),
        seq_number,
        effective_time,
        timeout,
        payload,
    )
        .into();

    let response = client
        .send_transaction(DEFAULT_NETWORK_ID, &block_item)
        .await
        .context("Could not send transaction.")?;
    anyhow::ensure!(response, "Submission of the update instruction failed.");
    Ok(block_item.hash())
}

async fn check_update_status(submission_id: hashes::TransactionHash, mut client: endpoints::Client) -> Result<()> {
    // wait until it's finalized.
    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(1));
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
            TransactionStatus::Received => {
                log::debug!("Submission is received.")
            }
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

    let client = reqwest::Client::new();
    log::debug!("Polling for exchange rate");
    let rate = pull_exchange_rate(client).await?;
    log::info!("New exchange rate polled: {:#?}", rate);
    let summary = get_block_summary(node_client.clone()).await?;
    let signer = get_signer(app.governance_keys, &summary).await?;
    log::info!("keys loaded");
    let submission_id = send_update(summary, signer, rate, node_client.clone()).await?;
    log::info!("sent update with submission id: {:#?}", submission_id);
    check_update_status(submission_id, node_client.clone()).await?;
    Ok(())
}
