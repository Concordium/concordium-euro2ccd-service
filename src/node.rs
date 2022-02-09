use crate::{
    config::{CHECK_SUBMISSION_STATUS_INTERVAL, RETRY_SUBMISSION_INTERVAL, UPDATE_EXPIRY_OFFSET},
    prometheus::Stats,
};
use anyhow::Context;
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

pub async fn get_block_summary(mut node_client: endpoints::Client) -> anyhow::Result<BlockSummary> {
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

/**
 * Sends an microCCD per Euro update, with the given exchange rate.
 * If it runs into issues, log the error and try again.
 * If the given node is not responding, then return None.
 * The given sequence number will be used initially, but a new one will be
 * requested, if the first attempt is not accepted. The returned sequence
 * number is the one used in the successful update.
 */
pub async fn send_update(
    stats: &Stats,
    mut seq_number: UpdateSequenceNumber,
    signer: &[(UpdateKeysIndex, KeyPair)],
    exchange_rate: ExchangeRate,
    mut client: endpoints::Client,
) -> Option<(hashes::TransactionHash, UpdateSequenceNumber)> {
    let mut get_new_seq_number = false;

    let mut interval = interval(Duration::from_secs(RETRY_SUBMISSION_INTERVAL));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;

        if get_new_seq_number {
            let new_summary = match get_block_summary(client.clone()).await {
                Ok(o) => o,
                Err(e) => {
                    log::error!("Unable to pull new sequence number due to: {}", e);
                    // The only reason this should fail is a connection issue.
                    return None;
                }
            };
            seq_number = new_summary.updates.update_queues.micro_gtu_per_euro.next_sequence_number;
        }
        // Construct the block item again. This sets the expiry from now so it is
        // necessary to reconstruct on each attempt.
        let block_item = construct_block_item(seq_number, signer, exchange_rate);
        match client.send_transaction(DEFAULT_NETWORK_ID, &block_item).await {
            Ok(true) => {
                stats.reset_update_attempts();
                return Some((block_item.hash(), seq_number));
            }
            Ok(false) => {
                stats.increment_update_attempts();
                log::error!("Sending update was rejected, id: {}.", block_item.hash());
                // We assume that the reason for rejection is an incorrect sequence number
                // (because it is the only one we can solve)
                get_new_seq_number = true;
            }
            Err(endpoints::RPCError::CallError(status)) => {
                stats.increment_update_attempts();
                match status.code() {
                    tonic::Code::Internal
                    | tonic::Code::FailedPrecondition
                    | tonic::Code::PermissionDenied
                    | tonic::Code::Aborted
                    | tonic::Code::Unavailable
                    | tonic::Code::Unknown => {
                        log::error!("Unable to reach current node during update");
                        return None;
                    }
                    code => {
                        log::error!("RPC error occurred while sending update: {}", code);
                        get_new_seq_number = true;
                    }
                }
            }
            Err(e) => {
                stats.increment_update_attempts();
                // This case could happen for a number of reasons. Currently the node
                // responds with this for different reasons and we cannot fully determine what
                // we should do based on the status. If the node ever responds more precisely
                // then we can revise this to be smarter about it.
                log::error!("Error occurred while sending update: {}", e);
                get_new_seq_number = true;
            }
        }
    }
}

pub async fn check_update_status(
    submission_id: hashes::TransactionHash,
    mut client: endpoints::Client,
) -> anyhow::Result<()> {
    let mut interval = interval(Duration::from_secs(CHECK_SUBMISSION_STATUS_INTERVAL));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
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

/**
 * Given a vector of endpoints, return the first one, which allows us to
 * connect to it. Returns an error if we are not able to connect to any of
 * the nodes.
 */
pub async fn get_node_client(
    endpoints: Vec<endpoints::Endpoint>,
    token: &str,
) -> anyhow::Result<endpoints::Client> {
    for node_ep in endpoints.into_iter() {
        if let Ok(client) = endpoints::Client::connect(node_ep, token.to_string()).await {
            return Ok(client);
        };
    }
    anyhow::bail!("Unable to connect to any node");
}
