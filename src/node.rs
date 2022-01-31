use anyhow::{Context, Result};
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
use crypto_common::types::{KeyPair, TransactionTime};
use tokio::time::{interval, Duration};

const CHECK_SUBMISSION_STATUS_INTERVAL: u64 = 5; // seconds
const RETRY_SUBMISSION_INTERVAL: u64 = 10; // seconds
const UPDATE_EXPIRY_OFFSET: u64 = 100; // seconds

pub async fn get_block_summary(mut node_client: endpoints::Client) -> Result<BlockSummary> {
    let consensus_status = node_client
        .get_consensus_status()
        .await
        .context("Could not obtain status of consensus.")?;

    let summary: BlockSummary = node_client
        .get_block_summary(&consensus_status.last_finalized_block)
        .await
        .context("Could not obtain last finalized block")?;
    Ok(summary)
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

pub async fn send_update(
    mut seq_number: UpdateSequenceNumber,
    signer: &[(UpdateKeysIndex, KeyPair)],
    exchange_rate: ExchangeRate,
    mut client: endpoints::Client,
) -> (hashes::TransactionHash, UpdateSequenceNumber) {
    let mut get_new_seq_number = false;

    let mut interval = interval(Duration::from_secs(RETRY_SUBMISSION_INTERVAL));
    loop {
        interval.tick().await;

        if get_new_seq_number {
            let new_summary = match get_block_summary(client.clone()).await {
                Ok(o) => o,
                Err(e) => {
                    log::error!("Unable to pull new sequence number due to: {:#?}", e);
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
            Err(e) => log::error!("Error occurred while sending update: {:#?}", e),
        }
    }
}

pub async fn check_update_status(
    submission_id: hashes::TransactionHash,
    mut client: endpoints::Client,
) -> Result<()> {
    let mut interval = interval(Duration::from_secs(CHECK_SUBMISSION_STATUS_INTERVAL));
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
