use std::time::Duration;

use anyhow::Result;
use common::block::Block;
use logos_blockchain_key_management_system_service::keys::Ed25519Key;
use logos_blockchain_zone_sdk::sequencer::WithdrawArg;

use crate::{
    block_publisher::{
        BlockPublisherTrait, CheckpointSink, FinalizedBlockSink, OnDepositEventSink,
        OnWithdrawEventSink, SequencerCheckpoint,
    },
    config::BedrockConfig,
};

pub type SequencerCoreWithMockClients = crate::SequencerCore<MockBlockPublisher>;

#[derive(Clone)]
pub struct MockBlockPublisher;

impl BlockPublisherTrait for MockBlockPublisher {
    async fn new(
        _config: &BedrockConfig,
        _bedrock_signing_key: Ed25519Key,
        _resubmit_interval: Duration,
        _initial_checkpoint: Option<SequencerCheckpoint>,
        _on_checkpoint: CheckpointSink,
        _on_finalized_block: FinalizedBlockSink,
        _on_deposit_event: OnDepositEventSink,
        _on_withdraw_event: OnWithdrawEventSink,
    ) -> Result<Self> {
        Ok(Self)
    }

    async fn publish_block(
        &self,
        _block: &Block,
        _bridge_withdrawals: Vec<WithdrawArg>,
    ) -> Result<()> {
        Ok(())
    }
}
