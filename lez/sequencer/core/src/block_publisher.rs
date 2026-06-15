use std::{pin::Pin, sync::Arc, time::Duration};

use anyhow::{Context as _, Result};
use common::block::Block;
use log::warn;
pub use logos_blockchain_core::mantle::ops::channel::MsgId;
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
use tokio::{sync::Mutex, task::JoinHandle};

use crate::config::BedrockConfig;

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
pub trait BlockPublisherTrait: Sized {
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
    async fn publish_block(&mut self, block: &Block) -> Result<()>;

    async fn wait_ready(&self);
}

/// Real block publisher backed by zone-sdk's `ZoneSequencer`.
//#[derive(Clone)]
pub struct ZoneSdkPublisher {
    sequencer: Arc<Mutex<ZoneSequencer<NodeHttpClient>>>,
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

        let sequencer = Arc::new(Mutex::new(ZoneSequencer::init_with_config(
            config.channel_id,
            bedrock_signing_key,
            node,
            zone_sdk_config,
            initial_checkpoint,
        )));

        let drive_sequencer = sequencer.clone();

        let drive_task = tokio::spawn(async move {
            loop {
                let event = {
                    let mut event_guard = drive_sequencer.lock().await;

                    let Some(event) = event_guard.next_event().await else {
                        continue;
                    };
                    event
                };

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
                                    if let Some(block_id) = block_id_from_inscription(&inscription)
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
        });

        Ok(Self {
            sequencer,
            _drive_task: Arc::new(DriveTaskGuard(drive_task)),
        })
    }

    async fn publish_block(&mut self, block: &Block) -> Result<()> {
        let data = borsh::to_vec(block).context("Failed to serialize block")?;
        let data_bounded = data
            .try_into()
            .context("Block data exceeds maximum allowed size")?;

        {
            let mut handle_guard = self.sequencer.lock().await;

            let _res = handle_guard
                .handle()
                .publish(data_bounded)
                .context("Failed to publish block")?;
        }

        Ok(())
    }

    async fn wait_ready(&self) {
        {
            let ready_guard = self.sequencer.lock().await;

            ready_guard
                .subscribe_ready()
                .wait_for(|val| *val)
                .await
                .expect("Channel should be alive");
        }
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
