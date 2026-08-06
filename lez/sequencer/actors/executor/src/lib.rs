//! Executor Actor performs the main logic of the Sequencer.

use anyhow::Result;
use common::{block::Block, transaction::LeeTransaction};
use kameo::{Actor, message::Message};
use lee_core::{
    BlockId,
    account::{Balance, Nonce},
};
use mempool::MemPoolHandle;
use sequencer_core::{
    SequencerCore, TransactionOrigin,
    block_publisher::{BlockPublisherTrait as _, ZoneSdkPublisher},
    config::SequencerConfig,
};

use crate::protocol::{
    GetAccount, GetAccountBalance, GetAccountNonces, GetAccountReply, GetBlock, GetBlockRange,
    GetChannelId, GetChannelIdReply, GetLastBlockId, GetProofsAndRoot, GetTransaction, Transaction,
};

pub mod protocol;

#[derive(Actor)]
pub struct ExecutorActor {
    sequencer: SequencerCore<ZoneSdkPublisher>,
    mempool_handle: MemPoolHandle<(TransactionOrigin, LeeTransaction)>,
}

impl ExecutorActor {
    pub async fn new(config: SequencerConfig) -> Self {
        let (sequencer, mempool_handle): (SequencerCore, _) =
            SequencerCore::start_from_config(config).await;

        Self {
            sequencer,
            mempool_handle,
        }
    }
}

impl Message<Transaction> for ExecutorActor {
    type Reply = ();

    async fn handle(
        &mut self,
        Transaction { transaction }: Transaction,
        _ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.mempool_handle
            .push((TransactionOrigin::User, transaction))
            .await
            .expect("Mempool is closed, this is a bug");
    }
}

impl Message<GetBlock> for ExecutorActor {
    type Reply = Result<Option<Block>>;

    async fn handle(
        &mut self,
        GetBlock { block_id }: GetBlock,
        _ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.sequencer
            .block_store()
            .get_block_at_id(block_id)
            .map_err(Into::into)
    }
}

impl Message<GetBlockRange> for ExecutorActor {
    type Reply = Result<Vec<Block>>;

    async fn handle(
        &mut self,
        GetBlockRange { range }: GetBlockRange,
        _ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        range
            .map_while(|block_id| {
                self.sequencer
                    .block_store()
                    .get_block_at_id(block_id)
                    .map_err(Into::into)
                    .transpose()
            })
            .collect::<Result<Vec<_>, _>>()
    }
}

impl Message<GetLastBlockId> for ExecutorActor {
    type Reply = Result<BlockId>;

    async fn handle(
        &mut self,
        GetLastBlockId: GetLastBlockId,
        _ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        Ok(self.sequencer.chain_height())
    }
}

impl Message<GetAccountBalance> for ExecutorActor {
    type Reply = Balance;

    async fn handle(
        &mut self,
        GetAccountBalance { account_id }: GetAccountBalance,
        _ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.sequencer
            .with_state(|state| state.get_account_by_id(account_id).balance)
    }
}

impl Message<GetTransaction> for ExecutorActor {
    type Reply = Option<(LeeTransaction, BlockId)>;

    async fn handle(
        &mut self,
        GetTransaction { tx_hash }: GetTransaction,
        _ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.sequencer
            .block_store()
            .get_transaction_by_hash(tx_hash)
    }
}

impl Message<GetAccountNonces> for ExecutorActor {
    type Reply = Vec<Nonce>;

    async fn handle(
        &mut self,
        GetAccountNonces { account_ids }: GetAccountNonces,
        _ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.sequencer.with_state(|state| {
            account_ids
                .into_iter()
                .map(|account_id| state.get_account_by_id(account_id).nonce)
                .collect()
        })
    }
}

impl Message<GetProofsAndRoot> for ExecutorActor {
    type Reply = (
        Vec<Option<lee_core::MembershipProof>>,
        lee_core::CommitmentSetDigest,
    );

    async fn handle(
        &mut self,
        GetProofsAndRoot { commitments }: GetProofsAndRoot,
        _ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.sequencer.with_state(|state| {
            let proofs = commitments
                .iter()
                .map(|commitment| state.get_proof_for_commitment(commitment))
                .collect();
            (proofs, state.commitment_root())
        })
    }
}

impl Message<GetAccount> for ExecutorActor {
    type Reply = GetAccountReply;

    async fn handle(
        &mut self,
        GetAccount { account_id }: GetAccount,
        _ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        GetAccountReply {
            account: self
                .sequencer
                .with_state(|state| state.get_account_by_id(account_id)),
        }
    }
}

impl Message<GetChannelId> for ExecutorActor {
    type Reply = GetChannelIdReply;

    async fn handle(
        &mut self,
        GetChannelId: GetChannelId,
        _ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        GetChannelIdReply {
            channel_id: *self.sequencer.block_publisher().channel_id().as_ref(),
        }
    }
}
