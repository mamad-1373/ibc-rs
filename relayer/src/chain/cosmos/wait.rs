use core::time::Duration;
use ibc::core::ics24_host::identifier::ChainId;
use ibc::events::IbcEvent;
use ibc::Height;
use itertools::Itertools;
use std::thread;
use std::time::Instant;
use tendermint_rpc::{HttpClient, Url};
use tracing::{info, trace};

use crate::chain::cosmos::query::tx::query_tx_response;
use crate::chain::cosmos::types::events::from_tx_response_event;
use crate::chain::cosmos::types::tx::{TxStatus, TxSyncResult};
use crate::error::Error;

const WAIT_BACKOFF: Duration = Duration::from_millis(300);

/// Given a vector of `TxSyncResult` elements,
/// each including a transaction response hash for one or more messages, periodically queries the chain
/// with the transaction hashes to get the list of IbcEvents included in those transactions.
pub async fn wait_for_block_commits(
    chain_id: &ChainId,
    rpc_client: &HttpClient,
    rpc_address: &Url,
    rpc_timeout: &Duration,
    tx_sync_results: &mut [TxSyncResult],
) -> Result<(), Error> {
    let start_time = Instant::now();

    let hashes = tx_sync_results
        .iter()
        .map(|res| res.response.hash.to_string())
        .join(", ");

    info!(
        id = %chain_id,
        "wait_for_block_commits: waiting for commit of tx hashes(s) {}",
        hashes
    );

    loop {
        let elapsed = start_time.elapsed();

        if all_tx_results_found(tx_sync_results) {
            trace!(
                id = %chain_id,
                "wait_for_block_commits: retrieved {} tx results after {}ms",
                tx_sync_results.len(),
                elapsed.as_millis(),
            );

            return Ok(());
        } else if &elapsed > rpc_timeout {
            return Err(Error::tx_no_confirmation());
        } else {
            thread::sleep(WAIT_BACKOFF);

            for tx_sync_result in tx_sync_results.iter_mut() {
                // ignore error
                let _ =
                    update_tx_sync_result(chain_id, rpc_client, rpc_address, tx_sync_result).await;
            }
        }
    }
}

async fn update_tx_sync_result(
    chain_id: &ChainId,
    rpc_client: &HttpClient,
    rpc_address: &Url,
    tx_sync_result: &mut TxSyncResult,
) -> Result<(), Error> {
    if let TxStatus::Pending { message_count } = tx_sync_result.status {
        let response =
            query_tx_response(rpc_client, rpc_address, &tx_sync_result.response.hash).await?;

        if let Some(response) = response {
            tx_sync_result.status = TxStatus::ReceivedResponse;

            if response.tx_result.code.is_err() {
                tx_sync_result.events = vec![
                    IbcEvent::ChainError(format!(
                        "deliver_tx for {} reports error: code={:?}, log={:?}",
                        response.hash, response.tx_result.code, response.tx_result.log
                    ));
                    message_count
                ];
            } else {
                let height = Height::new(chain_id.version(), u64::from(response.height)).unwrap();

                tx_sync_result.events = response
                    .tx_result
                    .events
                    .iter()
                    .flat_map(|event| from_tx_response_event(height, event).into_iter())
                    .collect::<Vec<_>>();
            }
        }
    }

    Ok(())
}

fn all_tx_results_found(tx_sync_results: &[TxSyncResult]) -> bool {
    tx_sync_results
        .iter()
        .all(|r| matches!(r.status, TxStatus::ReceivedResponse))
}
