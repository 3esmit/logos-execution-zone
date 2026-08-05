#![expect(
    clippy::tests_outside_test_module,
    reason = "top-level test functions are conventional for integration tests"
)]

//! Cross-zone round trip with the indexer in the loop (Option B). A ping on zone
//! A is delivered to zone B, and zone B's indexer independently re-derives the
//! injected dispatch from zone A's finalized blocks before applying it. The
//! payload landing in the indexer's state proves verification passed; a forgery
//! would have halted the indexer instead.

use std::{net::SocketAddr, time::Duration};

use anyhow::{Context as _, Result};
use common::transaction::LeeTransaction;
use cross_zone_outbox_core::outbox_pda;
use indexer_service_rpc::RpcClient as _;
use integration_tests::{
    config::{self, SequencerPartialConfig},
    indexer_client::IndexerClient,
    setup::{setup_bedrock_node, setup_indexer, setup_sequencer},
};
use lee::{AccountId, PublicTransaction, public_transaction::Message};
use lee_core::program::ProgramId;
use ping_core::{ReceiverInstruction, SenderInstruction, ping_record_pda};
use sequencer_core::config::{CrossZoneConfig, CrossZonePeer};
use sequencer_service_rpc::{RpcClient as _, SequencerClient, SequencerClientBuilder};
use tokio::{test, time::Instant};

const DELIVERY_TIMEOUT: Duration = Duration::from_secs(600);
const DELIVERY_POLL_INTERVAL: Duration = Duration::from_secs(3);
const DELIVERY_RPC_TIMEOUT: Duration = Duration::from_secs(10);
const ZONE_LIVE_TIMEOUT: Duration = Duration::from_secs(360);
const MIN_BLOCK_ID: u64 = 2;
const PING_PAYLOAD: &[u8] = b"hello-verified-zone";

#[derive(Debug, Default)]
struct DeliveryObservation {
    polls: u32,
    last_finalized_block: Option<u64>,
    last_payload_bytes: usize,
    last_error: Option<String>,
}

impl DeliveryObservation {
    fn timeout_message(&self, elapsed: Duration) -> String {
        format!(
            "Zone B's indexer did not record the verified payload after {elapsed:?}; polls={}, last finalized block={:?}, payload bytes={}, last error={:?}",
            self.polls, self.last_finalized_block, self.last_payload_bytes, self.last_error
        )
    }
}

#[test]
async fn indexer_verifies_and_delivers_cross_zone_ping() -> Result<()> {
    // Declared first so it outlives both zones (drops run in reverse order).
    let (_bedrock, bedrock_addr) = setup_bedrock_node()
        .await
        .context("Failed to set up shared Bedrock node")?;

    let partial = SequencerPartialConfig::default();
    let channel_a = config::bedrock_channel_id();
    let channel_b = config::bedrock_channel_id_b();
    let zone_a: [u8; 32] = *channel_a.as_ref();
    let zone_b: [u8; 32] = *channel_b.as_ref();

    let receiver_id = programs::ping_receiver().id();
    let cross_zone = CrossZoneConfig {
        peers: vec![CrossZonePeer {
            channel_id: zone_a,
            allowed_targets: vec![receiver_id],
            expected_block_signing_pubkey: None,
        }],
    };

    // Zone A: source. Zone B: destination, with the watcher on its sequencer and
    // the verifier on its indexer.
    let (seq_a, _seq_a_home) = setup_sequencer(partial, bedrock_addr, vec![], channel_a, None)
        .await
        .context("Failed to set up zone A sequencer")?;
    let (seq_b, _seq_b_home) = setup_sequencer(
        partial,
        bedrock_addr,
        vec![],
        channel_b,
        Some(cross_zone.clone()),
    )
    .await
    .context("Failed to set up zone B sequencer")?;
    let (idx_b, _idx_b_home) = setup_indexer(bedrock_addr, channel_b, Some(cross_zone))
        .await
        .context("Failed to set up zone B indexer")?;

    // Let both sequencers produce and both indexers finalize their local chain
    // before introducing the cross-zone dispatch. Without this barrier the
    // first ping can race indexer startup and leave the verifier with no
    // finalized source block to inspect.
    let (seq_a_client, seq_b_client) = (
        sequencer_client(seq_a.addr())?,
        sequencer_client(seq_b.addr())?,
    );
    let idx_b_client = indexer_client(idx_b.addr()).await?;
    tokio::try_join!(
        wait_until_sequencer_live("A", &seq_a_client),
        wait_until_zone_live("B", &seq_b_client, &idx_b_client),
    )?;

    // Submit the ping on zone A, addressed to ping_receiver on zone B.
    let ping = build_ping_tx(zone_b, receiver_id);
    seq_a_client
        .send_transaction(ping)
        .await
        .context("Failed to submit ping on zone A")?;

    // Wait until zone B's indexer records the delivered payload. The indexer only
    // applies the dispatch after re-deriving and verifying it.
    let record_id = ping_record_pda(receiver_id);
    let delivered = wait_for_indexer_delivery(&idx_b_client, record_id).await?;
    assert_eq!(
        delivered, PING_PAYLOAD,
        "Zone B's indexer must record the verified cross-zone payload"
    );
    Ok(())
}

fn build_ping_tx(target_zone: [u8; 32], receiver_id: ProgramId) -> LeeTransaction {
    let outbox_id = programs::cross_zone_outbox().id();
    let ordinal = 0;

    let words = risc0_zkvm::serde::to_vec(&ReceiverInstruction::Record {
        payload: PING_PAYLOAD.to_vec(),
    })
    .expect("serialize ping instruction");
    let payload: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();

    let send = SenderInstruction::Send {
        outbox_program_id: outbox_id,
        target_zone,
        target_program_id: receiver_id,
        target_accounts: vec![ping_record_pda(receiver_id).into_value()],
        payload,
        ordinal,
    };

    let outbox_account = outbox_pda(outbox_id, &target_zone, ordinal);
    let message = Message::try_new(
        programs::ping_sender().id(),
        vec![outbox_account],
        vec![],
        send,
    )
    .expect("build ping message");
    LeeTransaction::Public(PublicTransaction::new(
        message,
        lee::public_transaction::WitnessSet::from_raw_parts(vec![]),
    ))
}

fn sequencer_client(addr: SocketAddr) -> Result<SequencerClient> {
    let url = config::addr_to_url(config::UrlProtocol::Http, addr)
        .context("Failed to build sequencer URL")?;
    SequencerClientBuilder::default()
        .build(url)
        .context("Failed to build sequencer client")
}

async fn indexer_client(addr: SocketAddr) -> Result<IndexerClient> {
    let indexer_url = config::addr_to_url(config::UrlProtocol::Ws, addr)
        .context("Failed to build indexer URL")?;
    IndexerClient::new(&indexer_url)
        .await
        .context("Failed to build indexer client")
}

async fn wait_until_zone_live(
    label: &str,
    sequencer: &SequencerClient,
    indexer: &IndexerClient,
) -> Result<()> {
    let wait = async {
        loop {
            if sequencer.get_last_block_id().await? >= MIN_BLOCK_ID {
                break;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }

        let target = sequencer.get_last_block_id().await?;
        loop {
            let finalized = indexer.get_last_finalized_block_id().await?.unwrap_or(0);
            if finalized >= target {
                log::info!(
                    "Zone {label} ready: sequencer at {target}, indexer finalized {finalized}"
                );
                return Ok::<(), anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    };

    tokio::time::timeout(ZONE_LIVE_TIMEOUT, wait)
        .await
        .with_context(|| format!("Zone {label} did not become live within {ZONE_LIVE_TIMEOUT:?}"))?
}

/// Wait for a source sequencer to publish beyond its genesis block.
///
/// The source indexer is not part of this test's assertion: the destination
/// indexer verifies the source block directly from Bedrock's finalized stream.
async fn wait_until_sequencer_live(label: &str, sequencer: &SequencerClient) -> Result<()> {
    let wait = async {
        loop {
            if sequencer.get_last_block_id().await? >= MIN_BLOCK_ID {
                log::info!("Zone {label} sequencer live");
                return Ok::<(), anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    };

    tokio::time::timeout(ZONE_LIVE_TIMEOUT, wait)
        .await
        .with_context(|| {
            format!("Zone {label} sequencer did not become live within {ZONE_LIVE_TIMEOUT:?}")
        })?
}

/// Polls zone B's indexer until the ping record PDA holds a payload.
async fn wait_for_indexer_delivery(
    indexer: &IndexerClient,
    record_id: AccountId,
) -> Result<Vec<u8>> {
    let account_id = indexer_service_protocol::AccountId {
        value: record_id.into_value(),
    };
    let started = Instant::now();
    let deadline = started
        .checked_add(DELIVERY_TIMEOUT)
        .context("failed to calculate indexer delivery deadline")?;
    let mut observation = DeliveryObservation::default();

    loop {
        observation.polls = observation.polls.saturating_add(1);
        let previous_finalized = observation.last_finalized_block;

        match tokio::time::timeout(DELIVERY_RPC_TIMEOUT, indexer.get_last_finalized_block_id())
            .await
        {
            Ok(Ok(finalized)) => observation.last_finalized_block = finalized,
            Ok(Err(error)) => observation.last_error = Some(format!("finalized-head: {error}")),
            Err(_) => {
                observation.last_error = Some(format!(
                    "finalized-head timed out after {DELIVERY_RPC_TIMEOUT:?}"
                ));
            }
        }

        match tokio::time::timeout(
            DELIVERY_RPC_TIMEOUT,
            indexer_service_rpc::RpcClient::get_account(&**indexer, account_id),
        )
        .await
        {
            Ok(Ok(account)) => {
                let data = account.data.0;
                observation.last_payload_bytes = data.len();
                if !data.is_empty() {
                    return Ok(data);
                }
            }
            Ok(Err(error)) => observation.last_error = Some(format!("account query: {error}")),
            Err(_) => {
                observation.last_error = Some(format!(
                    "account query timed out after {DELIVERY_RPC_TIMEOUT:?}"
                ));
            }
        }

        if previous_finalized != observation.last_finalized_block || observation.polls == 1 {
            log::info!(
                "waiting for verified cross-zone payload: poll={}, finalized block={:?}, payload bytes={}",
                observation.polls,
                observation.last_finalized_block,
                observation.last_payload_bytes
            );
        }

        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            anyhow::bail!(observation.timeout_message(started.elapsed()));
        }
        tokio::time::sleep(DELIVERY_POLL_INTERVAL.min(remaining)).await;
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::DeliveryObservation;

    #[test]
    fn timeout_message_reports_last_indexer_observation() {
        let observation = DeliveryObservation {
            polls: 12,
            last_finalized_block: Some(42),
            last_payload_bytes: 0,
            last_error: Some("account query timed out".to_owned()),
        };

        let message = observation.timeout_message(Duration::from_secs(36));
        assert!(message.contains("polls=12"));
        assert!(message.contains("last finalized block=Some(42)"));
        assert!(message.contains("payload bytes=0"));
        assert!(message.contains("account query timed out"));
    }
}
