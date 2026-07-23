use std::time::Duration;

use anyhow::Result;
use common::block::Block;
use futures::Stream;
use logos_blockchain_core::mantle::ops::channel::{ChannelId, MsgId};
use logos_blockchain_key_management_system_service::keys::Ed25519Key;
use logos_blockchain_zone_sdk::{Slot, ZoneMessage, sequencer::WithdrawArg};
use tokio_util::sync::CancellationToken;

use crate::{
    block_publisher::{
        BlockPublisherTrait, CheckpointSink, FinalizedBlockSink, OnDepositEventSink, OnFollowSink,
        OnWithdrawEventSink, SequencerCheckpoint,
    },
    config::BedrockConfig,
};

pub type SequencerCoreWithMockClients = crate::SequencerCore<MockBlockPublisher>;

#[derive(Clone)]
pub struct MockBlockPublisher {
    channel_id: ChannelId,
    // Never cancelled: the mock driver never dies.
    driver_cancellation: CancellationToken,
    /// Canned channel frontier returned by [`Self::channel_tip_slot`].
    tip_slot: Option<Slot>,
    /// Canned finalized channel history returned by [`Self::read_channel_after`].
    messages: Vec<(ZoneMessage, Slot)>,
}

impl MockBlockPublisher {
    /// Builds a mock publisher backed by a canned channel, for reconstruction
    /// and consistency tests. The default (via [`BlockPublisherTrait::new`])
    /// serves an empty channel.
    #[must_use]
    pub fn with_canned_channel(
        channel_id: ChannelId,
        tip_slot: Option<Slot>,
        messages: Vec<(ZoneMessage, Slot)>,
    ) -> Self {
        Self {
            channel_id,
            driver_cancellation: CancellationToken::new(),
            tip_slot,
            messages,
        }
    }
}

impl BlockPublisherTrait for MockBlockPublisher {
    async fn new(
        config: &BedrockConfig,
        _bedrock_signing_key: Ed25519Key,
        _resubmit_interval: Duration,
        _initial_checkpoint: Option<SequencerCheckpoint>,
        _on_checkpoint: CheckpointSink,
        _on_finalized_block: FinalizedBlockSink,
        _on_deposit_event: OnDepositEventSink,
        _on_withdraw_event: OnWithdrawEventSink,
        _on_follow: OnFollowSink,
    ) -> Result<Self> {
        Ok(Self {
            channel_id: config.channel_id,
            driver_cancellation: CancellationToken::new(),
            tip_slot: None,
            messages: Vec::new(),
        })
    }

    async fn publish_block(
        &self,
        block: &Block,
        _bridge_withdrawals: Vec<WithdrawArg>,
    ) -> Result<MsgId> {
        // Deterministic per-block id so head dedup behaves in tests.
        //
        // TODO: should we allow more "mockability" here?
        Ok(MsgId::from(block.header.hash.0))
    }

    fn channel_id(&self) -> ChannelId {
        self.channel_id
    }

    fn is_our_turn(&self) -> bool {
        true
    }

    fn driver_cancellation(&self) -> CancellationToken {
        self.driver_cancellation.clone()
    }

    async fn channel_tip_slot(&self) -> Result<Option<Slot>> {
        Ok(self.tip_slot)
    }

    async fn read_channel_after(
        &self,
        after_slot: Option<Slot>,
    ) -> Result<impl Stream<Item = (ZoneMessage, Slot)> + '_> {
        // Mirror `next_messages`: `after_slot` is exclusive.
        let messages = self
            .messages
            .iter()
            .filter(move |(_, slot)| after_slot.is_none_or(|after| *slot > after))
            .cloned();
        Ok(futures::stream::iter(messages))
    }
}
