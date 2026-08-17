use std::collections::BTreeMap;

use bytesize::ByteSize;
use common::transaction::LeeTransaction;
use jsonrpsee::{
    core::async_trait,
    types::{ErrorCode, ErrorObjectOwned},
};
use kameo::actor::ActorRef;
use log::{error, warn};
use sequencer_core::{block_publisher::BlockPublisherTrait, gossip::GossipTxPublisher};
use sequencer_service_protocol::{
    Account, AccountId, Block, BlockId, ChannelId, Commitment, CommitmentSetDigest,
    CrossZoneDeadLetter, CrossZoneDeadLetterReport, HashType, LocalBlockHeaderReceiptV1,
    LocalPublicBlockHistoryPageV1, LocalPublicBlockHistoryRequestV1, LocalPublicBlockV1,
    LocalPublicTransactionReceiptV1, LocalPublicTransactionV1,
    MAX_LOCAL_PUBLIC_BLOCK_HISTORY_BLOCKS, MAX_LOCAL_PUBLIC_TRANSACTION_RECEIPT_CONFIRMATIONS,
    MembershipProof, Nonce, ProgramId,
};
use sequencer_service_rpc::TransactionLookupResponse;
use sha2::{Digest as _, Sha256};

pub struct Service<BP: BlockPublisherTrait + Send + 'static> {
    executor_ref: ActorRef<sequencer_executor_actor::ExecutorActor<BP>>,
    max_block_size: ByteSize,
    gossip_tx_publisher: Option<GossipTxPublisher>,
}

impl<BP: BlockPublisherTrait + Send + 'static> Service<BP> {
    pub fn new(
        executor_ref: ActorRef<sequencer_executor_actor::ExecutorActor<BP>>,
        max_block_size: ByteSize,
        gossip_tx_publisher: Option<GossipTxPublisher>,
    ) -> Self {
        sequencer_rpc_server_actor_metrics::init();

        Self {
            executor_ref,
            max_block_size,
            gossip_tx_publisher,
        }
    }

    async fn load_block_at_id(&self, block_id: BlockId) -> Result<Block, ErrorObjectOwned> {
        self.executor_ref
            .ask(sequencer_executor_actor::protocol::GetBlock { block_id })
            .await
            .map_err(internal_error)?
            .ok_or_else(|| {
                ErrorObjectOwned::owned(
                    ErrorCode::InvalidParams.code(),
                    format!("Block with id {block_id} not found"),
                    None::<()>,
                )
            })
    }
}

#[async_trait]
impl<BP: BlockPublisherTrait + Send + 'static> sequencer_service_rpc::RpcServer for Service<BP> {
    async fn send_transaction(&self, tx: LeeTransaction) -> Result<HashType, ErrorObjectOwned> {
        sequencer_rpc_server_actor_metrics::increment_submitted_transactions_total();

        let tx_hash = tx.hash();

        let res = async move {
            // Reserve ~200 bytes for block header overhead
            const BLOCK_HEADER_OVERHEAD: u64 = 200;

            let encoded_tx =
                borsh::to_vec(&tx).expect("Transaction borsh serialization should not fail");
            let tx_size =
                u64::try_from(encoded_tx.len()).expect("Transaction size should fit in u64");

            let max_tx_size = self
                .max_block_size
                .as_u64()
                .saturating_sub(BLOCK_HEADER_OVERHEAD);

            if tx_size > max_tx_size {
                return Err(ErrorObjectOwned::owned(
                    ErrorCode::InvalidParams.code(),
                    format!("Transaction too large: size {tx_size}, max {max_tx_size}"),
                    None::<()>,
                ));
            }

            let authenticated_tx = tx
                .transaction_stateless_check()
                .inspect_err(|err| warn!("Error at pre_check {err:#?}"))
                .map_err(|err| {
                    ErrorObjectOwned::owned(
                        ErrorCode::InvalidParams.code(),
                        format!("{err:?}"),
                        None::<()>,
                    )
                })?;

            // Sequencer-only programs (the cross-zone inbox) are injected by the
            // watcher; a user must not invoke them top-level, or anyone could forge
            // an inbound cross-zone delivery. Chained user calls are already rejected
            // by the inbox guest's caller-is-none assertion.
            if let LeeTransaction::Public(public_tx) = &authenticated_tx
                && sequencer_core::is_sequencer_only_program(public_tx.message().program_id)
            {
                return Err(ErrorObjectOwned::owned(
                    ErrorCode::InvalidParams.code(),
                    "Program is sequencer-only and cannot be invoked by a user transaction"
                        .to_owned(),
                    None::<()>,
                ));
            }

            Ok(authenticated_tx)
        };

        let authenticated_tx = res.await.inspect_err(|err| {
            sequencer_rpc_server_actor_metrics::increment_before_mempool_failed_transactions_total(
            );
            error!("Transaction failed before reaching mempool: {err:#?}");
        })?;

        // Publish to the gossip mesh before the local mempool admission so a
        // full mempool doesn't delay propagation.
        if let Some(publisher) = &self.gossip_tx_publisher {
            publisher.publish(authenticated_tx.clone());
        }

        self.executor_ref
            .ask(sequencer_executor_actor::protocol::Transaction {
                transaction: authenticated_tx,
            })
            .await
            .map_err(internal_error)?;

        Ok(tx_hash)
    }

    async fn check_health(&self) -> Result<(), ErrorObjectOwned> {
        Ok(())
    }

    async fn get_block(&self, block_id: BlockId) -> Result<Option<Block>, ErrorObjectOwned> {
        self.executor_ref
            .ask(sequencer_executor_actor::protocol::GetBlock { block_id })
            .await
            .map_err(internal_error)
    }

    async fn get_block_range(
        &self,
        start_block_id: BlockId,
        end_block_id: BlockId,
    ) -> Result<Vec<Block>, ErrorObjectOwned> {
        self.executor_ref
            .ask(sequencer_executor_actor::protocol::GetBlockRange {
                range: (start_block_id..=end_block_id),
            })
            .await
            .map_err(internal_error)
    }

    async fn get_last_block_id(&self) -> Result<BlockId, ErrorObjectOwned> {
        self.executor_ref
            .ask(sequencer_executor_actor::protocol::GetLastBlockId)
            .await
            .map_err(internal_error)
    }

    async fn get_account_balance(&self, account_id: AccountId) -> Result<u128, ErrorObjectOwned> {
        self.executor_ref
            .ask(sequencer_executor_actor::protocol::GetAccountBalance { account_id })
            .await
            .map_err(internal_error)
    }

    async fn get_transaction(
        &self,
        tx_hash: HashType,
    ) -> Result<TransactionLookupResponse, ErrorObjectOwned> {
        self.executor_ref
            .ask(sequencer_executor_actor::protocol::GetTransaction { tx_hash })
            .await
            .map(|transaction| {
                TransactionLookupResponse(transaction.map(|(transaction, _block_id)| transaction))
            })
            .map_err(internal_error)
    }

    async fn get_local_public_transaction_receipt(
        &self,
        tx_hash: HashType,
        confirmation_depth: u8,
    ) -> Result<Option<LocalPublicTransactionReceiptV1>, ErrorObjectOwned> {
        validate_confirmation_depth(confirmation_depth)?;

        let Some((transaction, inclusion_block_id)) = self
            .executor_ref
            .ask(sequencer_executor_actor::protocol::GetTransaction { tx_hash })
            .await
            .map_err(internal_error)?
        else {
            return Ok(None);
        };
        let LeeTransaction::Public(public_transaction) = transaction else {
            return Ok(None);
        };

        let tip_id = self
            .executor_ref
            .ask(sequencer_executor_actor::protocol::GetLastBlockId)
            .await
            .map_err(internal_error)?;
        let confirmation_tip_id = inclusion_block_id
            .checked_add(u64::from(confirmation_depth))
            .ok_or_else(confirmation_height_overflow)?;
        if tip_id < confirmation_tip_id {
            return Ok(None);
        }

        let inclusion_block = self.load_block_at_id(inclusion_block_id).await?;
        let inclusion =
            local_inclusion_header_receipt(&inclusion_block, inclusion_block_id, tx_hash)?;

        let mut confirmation_chain = Vec::with_capacity(usize::from(confirmation_depth));
        let mut previous_block_hash = inclusion.block_hash;
        for successor_offset in 1..=u64::from(confirmation_depth) {
            let block_id = inclusion_block_id
                .checked_add(successor_offset)
                .ok_or_else(confirmation_height_overflow)?;
            let block = self.load_block_at_id(block_id).await?;
            let header = local_confirmation_header_receipt(&block, block_id, previous_block_hash)?;
            previous_block_hash = header.block_hash;
            confirmation_chain.push(header);
        }

        build_local_public_transaction_receipt(
            tx_hash,
            &public_transaction,
            inclusion,
            confirmation_chain,
        )
        .map(Some)
    }

    async fn get_local_public_block_history(
        &self,
        request: LocalPublicBlockHistoryRequestV1,
    ) -> Result<LocalPublicBlockHistoryPageV1, ErrorObjectOwned> {
        let genesis_block_id = self
            .executor_ref
            .ask(sequencer_executor_actor::protocol::GetGenesisBlockId)
            .await
            .map_err(internal_error)?;
        let mut effective_request = request;
        effective_request.start_block_id = normalize_local_public_block_history_start(
            effective_request.start_block_id,
            genesis_block_id,
        );
        validate_local_public_block_history_request(&effective_request, genesis_block_id)?;

        let snapshot_tip_id = self
            .executor_ref
            .ask(sequencer_executor_actor::protocol::GetLastBlockId)
            .await
            .map_err(internal_error)?;
        let snapshot_tip = local_block_header_receipt(
            &self.load_block_at_id(snapshot_tip_id).await?,
            snapshot_tip_id,
        )?;
        if effective_request
            .expected_tip
            .as_ref()
            .is_some_and(|expected_tip| expected_tip != &snapshot_tip)
        {
            return Err(ErrorObjectOwned::owned(
                ErrorCode::InvalidParams.code(),
                "local public history snapshot changed".to_owned(),
                None::<()>,
            ));
        }

        if effective_request.start_block_id > snapshot_tip_id {
            return Ok(LocalPublicBlockHistoryPageV1 {
                snapshot_tip,
                blocks: Vec::new(),
                next_block_id: None,
            });
        }

        let expected_predecessor_hash = if effective_request.start_block_id > genesis_block_id {
            let predecessor_block_id =
                effective_request
                    .start_block_id
                    .checked_sub(1)
                    .ok_or_else(|| {
                        ErrorObjectOwned::owned(
                            ErrorCode::InternalError.code(),
                            "local public history predecessor block identifier underflows"
                                .to_owned(),
                            None::<()>,
                        )
                    })?;
            Some(
                local_block_header_receipt(
                    &self.load_block_at_id(predecessor_block_id).await?,
                    predecessor_block_id,
                )?
                .block_hash,
            )
        } else {
            None
        };

        let end_block_id = effective_request
            .start_block_id
            .checked_add(u64::from(effective_request.max_blocks).saturating_sub(1))
            .map(|candidate| candidate.min(snapshot_tip_id))
            .ok_or_else(|| {
                ErrorObjectOwned::owned(
                    ErrorCode::InvalidParams.code(),
                    "local public history block range overflows".to_owned(),
                    None::<()>,
                )
            })?;
        let blocks = self
            .executor_ref
            .ask(sequencer_executor_actor::protocol::GetBlockRange {
                range: effective_request.start_block_id..=end_block_id,
            })
            .await
            .map_err(internal_error)?;

        build_local_public_block_history_page(
            &effective_request,
            snapshot_tip,
            expected_predecessor_hash,
            &blocks,
        )
    }

    async fn get_accounts_nonces(
        &self,
        account_ids: Vec<AccountId>,
    ) -> Result<Vec<Nonce>, ErrorObjectOwned> {
        self.executor_ref
            .ask(sequencer_executor_actor::protocol::GetAccountNonces { account_ids })
            .await
            .map_err(internal_error)
    }

    async fn get_proofs_and_root(
        &self,
        commitments: Vec<Commitment>,
    ) -> Result<(Vec<Option<MembershipProof>>, CommitmentSetDigest), ErrorObjectOwned> {
        self.executor_ref
            .ask(sequencer_executor_actor::protocol::GetProofsAndRoot { commitments })
            .await
            .map_err(internal_error)
    }

    async fn get_account(&self, account_id: AccountId) -> Result<Account, ErrorObjectOwned> {
        self.executor_ref
            .ask(sequencer_executor_actor::protocol::GetAccount { account_id })
            .await
            .map(|reply| reply.account)
            .map_err(internal_error)
    }

    async fn get_program_ids(&self) -> Result<BTreeMap<String, ProgramId>, ErrorObjectOwned> {
        let mut program_ids = BTreeMap::new();
        program_ids.insert(
            "authenticated_transfer".to_owned(),
            programs::authenticated_transfer().id(),
        );
        program_ids.insert("token".to_owned(), programs::token().id());
        program_ids.insert("pinata".to_owned(), programs::pinata().id());
        program_ids.insert("amm".to_owned(), programs::amm().id());
        program_ids.insert(
            "privacy_preserving_circuit".to_owned(),
            lee::PRIVACY_PRESERVING_CIRCUIT_ID,
        );
        Ok(program_ids)
    }

    async fn list_programs(&self) -> Result<Vec<ProgramId>, ErrorObjectOwned> {
        self.executor_ref
            .ask(sequencer_executor_actor::protocol::GetProgramIds)
            .await
            .map_err(internal_error)
    }

    async fn get_channel_id(&self) -> Result<ChannelId, ErrorObjectOwned> {
        self.executor_ref
            .ask(sequencer_executor_actor::protocol::GetChannelId)
            .await
            .map(|reply| ChannelId(reply.channel_id))
            .map_err(internal_error)
    }

    async fn get_cross_zone_dead_letters(
        &self,
    ) -> Result<CrossZoneDeadLetterReport, ErrorObjectOwned> {
        let sequencer_executor_actor::protocol::GetCrossZoneDeadLettersReply {
            total_retired,
            retained,
        } = self
            .executor_ref
            .ask(sequencer_executor_actor::protocol::GetCrossZoneDeadLetters)
            .await
            .map_err(internal_error)?;

        Ok(CrossZoneDeadLetterReport {
            total_retired,
            retained: retained
                .into_iter()
                .map(|record| CrossZoneDeadLetter {
                    message_key: HashType(record.message_key),
                    src_zone: ChannelId(record.origin.src_zone),
                    src_block_id: record.origin.src_block_id,
                    src_tx_index: record.origin.src_tx_index,
                    failed_attempts: record.failed_attempts,
                    transaction_bytes: record.transaction_bytes,
                })
                .collect(),
        })
    }
}

fn validate_confirmation_depth(confirmation_depth: u8) -> Result<(), ErrorObjectOwned> {
    if confirmation_depth > MAX_LOCAL_PUBLIC_TRANSACTION_RECEIPT_CONFIRMATIONS {
        return Err(ErrorObjectOwned::owned(
            ErrorCode::InvalidParams.code(),
            format!(
                "confirmation depth exceeds maximum of {MAX_LOCAL_PUBLIC_TRANSACTION_RECEIPT_CONFIRMATIONS}"
            ),
            None::<()>,
        ));
    }
    Ok(())
}

fn validate_local_public_block_history_request(
    request: &LocalPublicBlockHistoryRequestV1,
    genesis_block_id: BlockId,
) -> Result<(), ErrorObjectOwned> {
    if request.start_block_id < genesis_block_id {
        return Err(ErrorObjectOwned::owned(
            ErrorCode::InvalidParams.code(),
            format!("local public history must start at or after genesis block {genesis_block_id}"),
            None::<()>,
        ));
    }
    if request.max_blocks == 0 || request.max_blocks > MAX_LOCAL_PUBLIC_BLOCK_HISTORY_BLOCKS {
        return Err(ErrorObjectOwned::owned(
            ErrorCode::InvalidParams.code(),
            format!(
                "local public history block count must be between 1 and {MAX_LOCAL_PUBLIC_BLOCK_HISTORY_BLOCKS}"
            ),
            None::<()>,
        ));
    }
    Ok(())
}

const fn normalize_local_public_block_history_start(
    requested_start_block_id: BlockId,
    genesis_block_id: BlockId,
) -> BlockId {
    if requested_start_block_id == 0 {
        genesis_block_id
    } else {
        requested_start_block_id
    }
}

fn confirmation_height_overflow() -> ErrorObjectOwned {
    ErrorObjectOwned::owned(
        ErrorCode::InternalError.code(),
        "confirmation block height overflow".to_owned(),
        None::<()>,
    )
}

fn local_block_header_receipt(
    block: &Block,
    expected_block_id: BlockId,
) -> Result<LocalBlockHeaderReceiptV1, ErrorObjectOwned> {
    if block.header.block_id != expected_block_id {
        return Err(ErrorObjectOwned::owned(
            ErrorCode::InternalError.code(),
            "local sequencer block identifier does not match its store location".to_owned(),
            None::<()>,
        ));
    }
    if block.recompute_hash() != block.header.hash {
        return Err(ErrorObjectOwned::owned(
            ErrorCode::InternalError.code(),
            "local sequencer block hash does not match its contents".to_owned(),
            None::<()>,
        ));
    }
    Ok(LocalBlockHeaderReceiptV1 {
        block_id: block.header.block_id,
        block_hash: block.header.hash,
        previous_block_hash: block.header.prev_block_hash,
        timestamp: block.header.timestamp,
    })
}

fn local_inclusion_header_receipt(
    block: &Block,
    expected_block_id: BlockId,
    transaction_hash: HashType,
) -> Result<LocalBlockHeaderReceiptV1, ErrorObjectOwned> {
    let header = local_block_header_receipt(block, expected_block_id)?;
    if !block
        .body
        .transactions
        .iter()
        .any(|transaction| transaction.hash() == transaction_hash)
    {
        return Err(ErrorObjectOwned::owned(
            ErrorCode::InternalError.code(),
            "local sequencer transaction is absent from its indexed inclusion block".to_owned(),
            None::<()>,
        ));
    }
    Ok(header)
}

fn local_confirmation_header_receipt(
    block: &Block,
    expected_block_id: BlockId,
    expected_previous_block_hash: HashType,
) -> Result<LocalBlockHeaderReceiptV1, ErrorObjectOwned> {
    let header = local_block_header_receipt(block, expected_block_id)?;
    if header.previous_block_hash != expected_previous_block_hash {
        return Err(ErrorObjectOwned::owned(
            ErrorCode::InternalError.code(),
            "local sequencer confirmation chain is discontinuous".to_owned(),
            None::<()>,
        ));
    }
    Ok(header)
}

fn build_local_public_block_history_page(
    request: &LocalPublicBlockHistoryRequestV1,
    snapshot_tip: LocalBlockHeaderReceiptV1,
    expected_predecessor_hash: Option<HashType>,
    blocks: &[Block],
) -> Result<LocalPublicBlockHistoryPageV1, ErrorObjectOwned> {
    if request
        .expected_tip
        .as_ref()
        .is_some_and(|expected_tip| expected_tip != &snapshot_tip)
    {
        return Err(ErrorObjectOwned::owned(
            ErrorCode::InvalidParams.code(),
            "local public history snapshot changed".to_owned(),
            None::<()>,
        ));
    }
    if blocks.len() > usize::from(request.max_blocks) {
        return Err(ErrorObjectOwned::owned(
            ErrorCode::InternalError.code(),
            "local public history response exceeds its requested block count".to_owned(),
            None::<()>,
        ));
    }

    let mut expected_block_id = request.start_block_id;
    let mut previous_hash = expected_predecessor_hash;
    let mut response_blocks = Vec::with_capacity(blocks.len());
    for block in blocks {
        let header = local_block_header_receipt(block, expected_block_id)?;
        if header.block_id > snapshot_tip.block_id {
            return Err(ErrorObjectOwned::owned(
                ErrorCode::InternalError.code(),
                "local public history block exceeds its snapshot tip".to_owned(),
                None::<()>,
            ));
        }
        if previous_hash.is_some_and(|hash| header.previous_block_hash != hash) {
            return Err(ErrorObjectOwned::owned(
                ErrorCode::InternalError.code(),
                "local public history block chain is discontinuous".to_owned(),
                None::<()>,
            ));
        }

        let public_transactions = block
            .body
            .transactions
            .iter()
            .filter_map(|transaction| {
                let LeeTransaction::Public(public_transaction) = transaction else {
                    return None;
                };
                Some(LocalPublicTransactionV1 {
                    transaction_hash: HashType(public_transaction.hash()),
                    transaction: Some(LeeTransaction::Public(public_transaction.clone())),
                    program_id: None,
                    account_ids: None,
                    instruction_data: None,
                })
            })
            .collect();

        previous_hash = Some(header.block_hash);
        expected_block_id = expected_block_id.checked_add(1).ok_or_else(|| {
            ErrorObjectOwned::owned(
                ErrorCode::InternalError.code(),
                "local public history block identifier overflow".to_owned(),
                None::<()>,
            )
        })?;
        response_blocks.push(LocalPublicBlockV1 {
            header,
            public_transactions,
        });
    }

    let next_block_id = response_blocks
        .last()
        .is_some_and(|block| block.header.block_id < snapshot_tip.block_id)
        .then_some(expected_block_id);
    Ok(LocalPublicBlockHistoryPageV1 {
        snapshot_tip,
        blocks: response_blocks,
        next_block_id,
    })
}

fn build_local_public_transaction_receipt(
    transaction_hash: HashType,
    public_transaction: &lee::PublicTransaction,
    inclusion: LocalBlockHeaderReceiptV1,
    confirmation_chain: Vec<LocalBlockHeaderReceiptV1>,
) -> Result<LocalPublicTransactionReceiptV1, ErrorObjectOwned> {
    if HashType(public_transaction.hash()) != transaction_hash {
        return Err(ErrorObjectOwned::owned(
            ErrorCode::InternalError.code(),
            "local sequencer transaction lookup returned a mismatched transaction".to_owned(),
            None::<()>,
        ));
    }

    let message = public_transaction.message();
    Ok(LocalPublicTransactionReceiptV1 {
        transaction_hash,
        program_id: message.program_id,
        account_ids: message.account_ids.clone(),
        instruction_word_count: u32::try_from(message.instruction_data.len()).map_err(
            |_conversion_error| {
                ErrorObjectOwned::owned(
                    ErrorCode::InternalError.code(),
                    "instruction word count exceeds u32".to_owned(),
                    None::<()>,
                )
            },
        )?,
        instruction_data_sha256: instruction_data_sha256(&message.instruction_data),
        inclusion,
        confirmation_chain,
    })
}

fn instruction_data_sha256(instruction_data: &[u32]) -> HashType {
    let mut hasher = Sha256::new();
    for word in instruction_data {
        hasher.update(word.to_le_bytes());
    }
    HashType(hasher.finalize().into())
}

fn internal_error(err: impl std::fmt::Display) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(ErrorCode::InternalError.code(), err.to_string(), None::<()>)
}
