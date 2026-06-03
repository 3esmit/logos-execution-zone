use std::{pin::Pin, sync::Arc, time::Duration};

use anyhow::{Context as _, Result};
use common::block::Block;
use log::{info, warn};
use logos_blockchain_core::mantle::ops::channel::inscribe::Inscription;
pub use logos_blockchain_key_management_system_service::keys::{Ed25519Key, ZkKey};
pub use logos_blockchain_zone_sdk::sequencer::SequencerCheckpoint;
use logos_blockchain_zone_sdk::{
    CommonHttpClient,
    adapter::NodeHttpClient,
    sequencer::{
        Event, SequencerConfig as ZoneSdkSequencerConfig, SequencerHandle, WithdrawArg,
        ZoneSequencer,
    },
    state::{DepositInfo, FinalizedOp, InscriptionInfo, WithdrawInfo},
};
use tokio::task::JoinHandle;

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

/// Sink for finalized Bedrock withdraw events.
pub type OnWithdrawEventSink =
    Box<dyn Fn(WithdrawInfo) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + 'static>;

#[expect(async_fn_in_trait, reason = "We don't care about Send/Sync here")]
pub trait BlockPublisherTrait: Clone {
    #[expect(
        clippy::too_many_arguments,
        reason = "Looks better than bundling all those callbacks into a struct"
    )]
    async fn new(
        config: &BedrockConfig,
        bedrock_signing_key: Ed25519Key,
        resubmit_interval: Duration,
        initial_checkpoint: Option<SequencerCheckpoint>,
        on_checkpoint: CheckpointSink,
        on_finalized_block: FinalizedBlockSink,
        on_deposit_event: OnDepositEventSink,
        on_withdraw_event: OnWithdrawEventSink,
    ) -> Result<Self>;

    /// Fire-and-forget publish. Zone-sdk drives the actual submission and
    /// retries internally; this just hands the payload off.
    async fn publish_block(&self, block: &Block, withdrawals: Vec<WithdrawArg>) -> Result<()>;
}

/// Real block publisher backed by zone-sdk's `ZoneSequencer`.
#[derive(Clone)]
pub struct ZoneSdkPublisher {
    handle: SequencerHandle<NodeHttpClient>,
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
        on_withdraw_event: OnWithdrawEventSink,
    ) -> Result<Self> {
        let basic_auth = config.auth.clone().map(Into::into);
        let node = NodeHttpClient::new(CommonHttpClient::new(basic_auth), config.node_url.clone());

        let zone_sdk_config = ZoneSdkSequencerConfig {
            resubmit_interval,
            ..ZoneSdkSequencerConfig::default()
        };

        let (mut sequencer, mut handle) = ZoneSequencer::init_with_config(
            config.channel_id,
            bedrock_signing_key,
            node,
            zone_sdk_config,
            initial_checkpoint,
        );

        let drive_task = tokio::spawn(async move {
            loop {
                let Some(event) = sequencer.next_event().await else {
                    continue;
                };
                match event {
                    Event::Checkpoint { checkpoint } => on_checkpoint(checkpoint),
                    Event::TxsFinalized { items } => {
                        for op in items.into_iter().flat_map(|item| item.ops) {
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
                                FinalizedOp::Withdraw(withdraw) => {
                                    on_withdraw_event(withdraw).await;
                                }
                            }
                        }
                    }
                    Event::ChannelUpdate { .. }
                    | Event::Published { .. }
                    | Event::Readiness { .. }
                    | Event::TurnNotification { .. } => {}
                }
            }
        });

        handle.wait_ready().await;

        Ok(Self {
            handle,
            _drive_task: Arc::new(DriveTaskGuard(drive_task)),
        })
    }

    async fn publish_block(&self, block: &Block, withdrawals: Vec<WithdrawArg>) -> Result<()> {
        let data = borsh::to_vec(block).context("Failed to serialize block")?;
        let data_bounded: Inscription = data
            .try_into()
            .context("Block data exceeds maximum allowed size")?;
        let data_byte_size = data_bounded.len();

        if withdrawals.is_empty() {
            self.handle
                .publish_message(data_bounded)
                .await
                .context("Failed to publish block")?;

            info!("Published block with the size of {data_byte_size} bytes");

            return Ok(());
        }

        let withdraw_count = withdrawals.len();
        self.handle
            .publish_atomic_withdraw(data_bounded, withdrawals)
            .await
            .context("Failed to publish block with withdrawals")?;

        info!(
            "Published block with the size of {data_byte_size} bytes and {withdraw_count} bridge withdrawals",
        );

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
