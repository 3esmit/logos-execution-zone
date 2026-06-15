use std::{pin::Pin, sync::Arc, time::Duration};

use anyhow::{Context as _, Result, anyhow};
use common::block::Block;
use log::warn;
pub use logos_blockchain_core::mantle::ops::channel::MsgId;
use logos_blockchain_core::mantle::ops::channel::inscribe::Inscription;
pub use logos_blockchain_key_management_system_service::keys::Ed25519Key;
pub use logos_blockchain_zone_sdk::sequencer::SequencerCheckpoint;
use logos_blockchain_zone_sdk::{
    CommonHttpClient,
    adapter::NodeHttpClient,
    sequencer::{
        DepositInfo, Event, FinalizedOp, InscriptionInfo,
        SequencerConfig as ZoneSdkSequencerConfig, ZoneSequencer,
    },
};
use tokio::{sync::mpsc, task::JoinHandle};

use crate::config::BedrockConfig;

/// Channel capacity for the publish inbox. One publish per produced block, drained
/// in microseconds by the drive task — 32 is huge headroom and just provides
/// backpressure if the drive task stalls (reconnect, long backfill).
const PUBLISH_INBOX_CAPACITY: usize = 32;

/// Sink for `Event::Published` checkpoints emitted by the drive task.
/// Caller is responsible for persistence (e.g. writing to rocksdb).
pub type CheckpointSink = Box<dyn Fn(SequencerCheckpoint) + Send + 'static>;

/// Sink for finalized L2 block ids derived from `Event::TxsFinalized` and
/// `Event::FinalizedInscriptions`. Caller is responsible for cleanup
/// (e.g. marking pending blocks as finalized in storage).
pub type FinalizedBlockSink = Box<dyn Fn(u64) + Send + 'static>;

/// Sink for finalized Bedrock deposit events.
pub type OnDepositEventSink =
    Box<dyn Fn(DepositInfo) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + 'static>;

#[expect(async_fn_in_trait, reason = "We don't care about Send/Sync here")]
pub trait BlockPublisherTrait: Clone {
    async fn new(
        config: &BedrockConfig,
        bedrock_signing_key: Ed25519Key,
        resubmit_interval: Duration,
        initial_checkpoint: Option<SequencerCheckpoint>,
        on_checkpoint: CheckpointSink,
        on_finalized_block: FinalizedBlockSink,
        on_deposit_event: OnDepositEventSink,
    ) -> Result<Self>;

    /// Fire-and-forget publish. Zone-sdk drives the actual submission and
    /// retries internally; this just hands the payload off.
    async fn publish_block(&self, block: &Block) -> Result<()>;
}

/// Real block publisher backed by zone-sdk's `ZoneSequencer`.
///
/// The sequencer is owned exclusively by the drive task — the new zone-sdk
/// `SequencerHandle` is a `&mut self` borrow, so any out-of-task access
/// requires routing requests through a channel. We send serialized
/// inscriptions over a bounded mpsc; the drive task `tokio::select!`s
/// between `next_event()` and the inbox, calling `sequencer.handle().publish(...)`
/// inline.
#[derive(Clone)]
pub struct ZoneSdkPublisher {
    publish_tx: mpsc::Sender<Inscription>,
    // Aborts the drive task when the last clone is dropped.
    _drive_task: Arc<DriveTaskGuard>,
}

struct DriveTaskGuard(JoinHandle<()>);

impl Drop for DriveTaskGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl BlockPublisherTrait for ZoneSdkPublisher {
    async fn new(
        config: &BedrockConfig,
        bedrock_signing_key: Ed25519Key,
        resubmit_interval: Duration,
        initial_checkpoint: Option<SequencerCheckpoint>,
        on_checkpoint: CheckpointSink,
        on_finalized_block: FinalizedBlockSink,
        on_deposit_event: OnDepositEventSink,
    ) -> Result<Self> {
        let basic_auth = config.auth.clone().map(Into::into);
        let node = NodeHttpClient::new(CommonHttpClient::new(basic_auth), config.node_url.clone());

        let zone_sdk_config = ZoneSdkSequencerConfig {
            resubmit_interval,
            ..ZoneSdkSequencerConfig::default()
        };

        let mut sequencer = ZoneSequencer::init_with_config(
            config.channel_id,
            bedrock_signing_key,
            node,
            zone_sdk_config,
            initial_checkpoint,
        );

        // Grab readiness receiver before moving the sequencer into the drive
        // task so we can await cold-start completion below.
        let mut ready_rx = sequencer.subscribe_ready();

        let (publish_tx, mut publish_rx) = mpsc::channel::<Inscription>(PUBLISH_INBOX_CAPACITY);

        let drive_task = tokio::spawn(async move {
            loop {
                #[expect(
                    clippy::integer_division_remainder_used,
                    reason = "tokio::select! expansion uses `%` for random branch selection"
                )]
                {
                    tokio::select! {
                        // Drain external publish requests by calling the
                        // borrowing handle — `&mut sequencer` is only
                        // available here.
                        Some(data) = publish_rx.recv() => {
                            if let Err(e) = sequencer.handle().publish(data) {
                                warn!("zone-sdk publish failed: {e:?}");
                            }
                        }
                        event = sequencer.next_event() => {
                            let Some(event) = event else { continue };
                            match event {
                                Event::BlocksProcessed {
                                    checkpoint,
                                    channel_update: _,
                                    finalized,
                                } => {
                                    on_checkpoint(checkpoint);
                                    for op in finalized.into_iter().flat_map(|item| item.ops) {
                                        match op {
                                            FinalizedOp::Inscription(inscription) => {
                                                if let Some(block_id) =
                                                    block_id_from_inscription(&inscription)
                                                {
                                                    on_finalized_block(block_id);
                                                }
                                            }
                                            FinalizedOp::Deposit(deposit) => {
                                                on_deposit_event(deposit).await;
                                            }
                                            FinalizedOp::Withdraw(_) => {}
                                        }
                                    }
                                }
                                Event::Ready | Event::TurnNotification { .. } => {}
                            }
                        }
                    }
                }
            }
        });

        // Wait for cold-start backfill to complete before returning so callers
        // can publish immediately (e.g. genesis block) without racing readiness.
        ready_rx
            .wait_for(|v| *v)
            .await
            .context("Zone-sdk readiness channel closed before becoming ready")?;

        Ok(Self {
            publish_tx,
            _drive_task: Arc::new(DriveTaskGuard(drive_task)),
        })
    }

    async fn publish_block(&self, block: &Block) -> Result<()> {
        let data = borsh::to_vec(block).context("Failed to serialize block")?;
        let data_bounded = data
            .try_into()
            .context("Block data exceeds maximum allowed size")?;

        self.publish_tx
            .send(data_bounded)
            .await
            .map_err(|_closed| anyhow!("Drive task is no longer running"))?;

        Ok(())
    }
}

/// Deserialize inscription payload as a `Block` and return it's`block_id`.
/// Bad payloads are logged and skipped.
fn block_id_from_inscription(inscription: &InscriptionInfo) -> Option<u64> {
    borsh::from_slice::<Block>(&inscription.payload)
        .inspect_err(|err| {
            warn!("Failed to deserialize block from inscription: {err:?}");
        })
        .ok()
        .map(|block| block.header.block_id)
}
