use std::{collections::BTreeMap, sync::Arc};

use common::transaction::LeeTransaction;
use jsonrpsee::{
    core::async_trait,
    types::{ErrorCode, ErrorObjectOwned},
};
use lee;
use log::warn;
use mempool::MemPoolHandle;
use sequencer_core::{
    DbError, SequencerCore, TransactionOrigin, block_publisher::BlockPublisherTrait,
};
use sequencer_service_protocol::{
    Account, AccountId, Block, BlockId, ChannelId, Commitment, CommitmentSetDigest, HashType,
    LocalBlockHeaderReceiptV1, LocalPublicTransactionReceiptV1,
    MAX_LOCAL_PUBLIC_TRANSACTION_RECEIPT_ACCOUNTS,
    MAX_LOCAL_PUBLIC_TRANSACTION_RECEIPT_CONFIRMATIONS, MembershipProof, Nonce, ProgramId,
};
use sha2::{Digest as _, Sha256};
use tokio::sync::Mutex;

const NOT_FOUND_ERROR_CODE: i32 = -31999;

pub struct SequencerService<BC: BlockPublisherTrait> {
    sequencer: Arc<Mutex<SequencerCore<BC>>>,
    mempool_handle: MemPoolHandle<(TransactionOrigin, LeeTransaction)>,
    max_block_size: u64,
}

impl<BC: BlockPublisherTrait> SequencerService<BC> {
    pub const fn new(
        sequencer: Arc<Mutex<SequencerCore<BC>>>,
        mempool_handle: MemPoolHandle<(TransactionOrigin, LeeTransaction)>,
        max_block_size: u64,
    ) -> Self {
        Self {
            sequencer,
            mempool_handle,
            max_block_size,
        }
    }
}

#[async_trait]
impl<BC: BlockPublisherTrait + Send + 'static> sequencer_service_rpc::RpcServer
    for SequencerService<BC>
{
    async fn send_transaction(&self, tx: LeeTransaction) -> Result<HashType, ErrorObjectOwned> {
        // Reserve ~200 bytes for block header overhead
        const BLOCK_HEADER_OVERHEAD: u64 = 200;

        let tx_hash = tx.hash();

        let encoded_tx =
            borsh::to_vec(&tx).expect("Transaction borsh serialization should not fail");
        let tx_size = u64::try_from(encoded_tx.len()).expect("Transaction size should fit in u64");

        let max_tx_size = self.max_block_size.saturating_sub(BLOCK_HEADER_OVERHEAD);

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
                "Program is sequencer-only and cannot be invoked by a user transaction".to_owned(),
                None::<()>,
            ));
        }

        self.mempool_handle
            .push((TransactionOrigin::User, authenticated_tx))
            .await
            .expect("Mempool is closed, this is a bug");

        Ok(tx_hash)
    }

    async fn check_health(&self) -> Result<(), ErrorObjectOwned> {
        Ok(())
    }

    async fn get_block(&self, block_id: BlockId) -> Result<Option<Block>, ErrorObjectOwned> {
        let sequencer = self.sequencer.lock().await;
        sequencer
            .block_store()
            .get_block_at_id(block_id)
            .map_err(|err| internal_error(&err))
    }

    async fn get_block_range(
        &self,
        start_block_id: BlockId,
        end_block_id: BlockId,
    ) -> Result<Vec<Block>, ErrorObjectOwned> {
        let sequencer = self.sequencer.lock().await;
        (start_block_id..=end_block_id)
            .map(|block_id| {
                let block = sequencer
                    .block_store()
                    .get_block_at_id(block_id)
                    .map_err(|err| internal_error(&err))?;
                block.ok_or_else(|| {
                    ErrorObjectOwned::owned(
                        NOT_FOUND_ERROR_CODE,
                        format!("Block with id {block_id} not found"),
                        None::<()>,
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()
    }

    async fn get_last_block_id(&self) -> Result<BlockId, ErrorObjectOwned> {
        let sequencer = self.sequencer.lock().await;
        Ok(sequencer.chain_height())
    }

    async fn get_account_balance(&self, account_id: AccountId) -> Result<u128, ErrorObjectOwned> {
        let sequencer = self.sequencer.lock().await;
        let account = sequencer.state().get_account_by_id(account_id);
        Ok(account.balance)
    }

    async fn get_transaction(
        &self,
        tx_hash: HashType,
    ) -> Result<Option<LeeTransaction>, ErrorObjectOwned> {
        let sequencer = self.sequencer.lock().await;
        Ok(sequencer
            .block_store()
            .get_transaction_by_hash(tx_hash)
            .map(|(transaction, _block_id)| transaction))
    }

    async fn get_local_public_transaction_receipt(
        &self,
        tx_hash: HashType,
        confirmation_depth: u8,
    ) -> Result<Option<LocalPublicTransactionReceiptV1>, ErrorObjectOwned> {
        validate_confirmation_depth(confirmation_depth)?;

        let sequencer = self.sequencer.lock().await;
        let Some((transaction, inclusion_block_id)) =
            sequencer.block_store().get_transaction_by_hash(tx_hash)
        else {
            return Ok(None);
        };
        let LeeTransaction::Public(public_transaction) = transaction else {
            return Ok(None);
        };
        let confirmation_tip_id = inclusion_block_id
            .checked_add(u64::from(confirmation_depth))
            .ok_or_else(|| {
                ErrorObjectOwned::owned(
                    ErrorCode::InternalError.code(),
                    "confirmation block height overflow".to_owned(),
                    None::<()>,
                )
            })?;
        if sequencer.chain_height() < confirmation_tip_id {
            return Ok(None);
        }

        let inclusion_block = block_at_id(&sequencer, inclusion_block_id)?;
        let mut confirmation_blocks = Vec::with_capacity(usize::from(confirmation_depth));
        if confirmation_depth > 0 {
            const FIRST_SUCCESSOR_OFFSET: u64 = 1;
            let first_confirmation_id = inclusion_block_id
                .checked_add(FIRST_SUCCESSOR_OFFSET)
                .ok_or_else(|| {
                    ErrorObjectOwned::owned(
                        ErrorCode::InternalError.code(),
                        "confirmation block height overflow".to_owned(),
                        None::<()>,
                    )
                })?;
            for block_id in first_confirmation_id..=confirmation_tip_id {
                confirmation_blocks.push(block_at_id(&sequencer, block_id)?);
            }
        }

        build_local_public_transaction_receipt(
            tx_hash,
            &public_transaction,
            inclusion_block_id,
            &inclusion_block,
            &confirmation_blocks,
        )
        .map(Some)
    }

    async fn get_accounts_nonces(
        &self,
        account_ids: Vec<AccountId>,
    ) -> Result<Vec<Nonce>, ErrorObjectOwned> {
        let sequencer = self.sequencer.lock().await;
        let nonces = account_ids
            .into_iter()
            .map(|account_id| sequencer.state().get_account_by_id(account_id).nonce)
            .collect();
        Ok(nonces)
    }

    async fn get_proofs_and_root(
        &self,
        commitments: Vec<Commitment>,
    ) -> Result<(Vec<Option<MembershipProof>>, CommitmentSetDigest), ErrorObjectOwned> {
        let sequencer = self.sequencer.lock().await;
        let state = sequencer.state();
        let proofs = commitments
            .iter()
            .map(|commitment| state.get_proof_for_commitment(commitment))
            .collect();
        Ok((proofs, state.commitment_root()))
    }

    async fn get_account(&self, account_id: AccountId) -> Result<Account, ErrorObjectOwned> {
        let sequencer = self.sequencer.lock().await;
        Ok(sequencer.state().get_account_by_id(account_id))
    }

    async fn get_program_ids(&self) -> Result<BTreeMap<String, ProgramId>, ErrorObjectOwned> {
        // TODO: Get programs from state
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
        let sequencer = self.sequencer.lock().await;
        Ok(sequencer.state().program_ids())
    }

    async fn get_channel_id(&self) -> Result<ChannelId, ErrorObjectOwned> {
        let channel_id = self.sequencer.lock().await.block_publisher().channel_id();
        Ok(ChannelId(*channel_id.as_ref()))
    }
}

fn internal_error(err: &DbError) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(ErrorCode::InternalError.code(), err.to_string(), None::<()>)
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

fn block_at_id<BC: BlockPublisherTrait>(
    sequencer: &SequencerCore<BC>,
    block_id: BlockId,
) -> Result<Block, ErrorObjectOwned> {
    sequencer
        .block_store()
        .get_block_at_id(block_id)
        .map_err(|err| internal_error(&err))?
        .ok_or_else(|| {
            ErrorObjectOwned::owned(
                NOT_FOUND_ERROR_CODE,
                format!("Block with id {block_id} not found"),
                None::<()>,
            )
        })
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
    })
}

fn build_local_public_transaction_receipt(
    transaction_hash: HashType,
    public_transaction: &lee::PublicTransaction,
    inclusion_block_id: BlockId,
    inclusion_block: &Block,
    confirmation_blocks: &[Block],
) -> Result<LocalPublicTransactionReceiptV1, ErrorObjectOwned> {
    if HashType(public_transaction.hash()) != transaction_hash {
        return Err(ErrorObjectOwned::owned(
            ErrorCode::InternalError.code(),
            "local sequencer transaction lookup returned a mismatched transaction".to_owned(),
            None::<()>,
        ));
    }

    let message = public_transaction.message();
    if message.account_ids.len() > MAX_LOCAL_PUBLIC_TRANSACTION_RECEIPT_ACCOUNTS {
        return Err(ErrorObjectOwned::owned(
            ErrorCode::InternalError.code(),
            format!(
                "public transaction has {} accounts; maximum receipt size is {MAX_LOCAL_PUBLIC_TRANSACTION_RECEIPT_ACCOUNTS}",
                message.account_ids.len()
            ),
            None::<()>,
        ));
    }

    let inclusion = local_block_header_receipt(inclusion_block, inclusion_block_id)?;
    if !inclusion_block
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

    let mut confirmation_chain = Vec::with_capacity(confirmation_blocks.len());
    let mut previous_hash = inclusion.block_hash;
    for (offset, block) in confirmation_blocks.iter().enumerate() {
        let offset = u64::try_from(offset).map_err(|_conversion_error| {
            ErrorObjectOwned::owned(
                ErrorCode::InternalError.code(),
                "confirmation block offset does not fit in u64".to_owned(),
                None::<()>,
            )
        })?;
        let expected_block_id = inclusion_block_id
            .checked_add(offset)
            .and_then(|block_id| block_id.checked_add(1))
            .ok_or_else(|| {
                ErrorObjectOwned::owned(
                    ErrorCode::InternalError.code(),
                    "confirmation block height overflow".to_owned(),
                    None::<()>,
                )
            })?;
        let header = local_block_header_receipt(block, expected_block_id)?;
        if header.previous_block_hash != previous_hash {
            return Err(ErrorObjectOwned::owned(
                ErrorCode::InternalError.code(),
                "local sequencer confirmation chain is discontinuous".to_owned(),
                None::<()>,
            ));
        }
        previous_hash = header.block_hash;
        confirmation_chain.push(header);
    }

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

#[cfg(test)]
mod tests {
    use common::{HashType, test_utils::produce_dummy_block};
    use lee::{
        AccountId, PublicTransaction,
        public_transaction::{Message, WitnessSet},
    };

    use super::{
        MAX_LOCAL_PUBLIC_TRANSACTION_RECEIPT_ACCOUNTS, build_local_public_transaction_receipt,
        instruction_data_sha256, validate_confirmation_depth,
    };

    fn public_transaction(
        account_ids: Vec<AccountId>,
        instruction_data: Vec<u32>,
    ) -> PublicTransaction {
        let message = Message::new_preserialized([7; 8], account_ids, vec![], instruction_data);
        PublicTransaction::new(message, WitnessSet::from_raw_parts(vec![]))
    }

    #[test]
    fn builds_a_bounded_local_transaction_receipt() {
        let transaction = public_transaction(
            vec![AccountId::new([3; 32]), AccountId::new([4; 32])],
            vec![0x1122_3344, 0x5566_7788],
        );
        let transaction_hash = HashType(transaction.hash());
        let inclusion = produce_dummy_block(
            7,
            Some(HashType([1; 32])),
            vec![common::transaction::LeeTransaction::Public(
                transaction.clone(),
            )],
        );
        let confirmation = produce_dummy_block(8, Some(inclusion.header.hash), vec![]);

        let receipt = build_local_public_transaction_receipt(
            transaction_hash,
            &transaction,
            7,
            &inclusion,
            &[confirmation],
        )
        .expect("valid local chain must produce a receipt");

        assert_eq!(receipt.transaction_hash, transaction_hash);
        assert_eq!(receipt.program_id, [7; 8]);
        assert_eq!(receipt.instruction_word_count, 2);
        assert_eq!(
            receipt.instruction_data_sha256,
            instruction_data_sha256(&[0x1122_3344, 0x5566_7788])
        );
        assert_eq!(receipt.inclusion.block_id, 7);
        assert_eq!(receipt.confirmation_chain.len(), 1);
        assert_eq!(receipt.confirmation_chain[0].block_id, 8);
        assert_eq!(
            receipt.confirmation_chain[0].previous_block_hash,
            receipt.inclusion.block_hash
        );
    }

    #[test]
    fn rejects_a_discontinuous_confirmation_chain() {
        let transaction = public_transaction(vec![], vec![]);
        let transaction_hash = HashType(transaction.hash());
        let inclusion = produce_dummy_block(
            7,
            Some(HashType([1; 32])),
            vec![common::transaction::LeeTransaction::Public(
                transaction.clone(),
            )],
        );
        let discontinuous_confirmation = produce_dummy_block(8, Some(HashType([9; 32])), vec![]);

        assert!(
            build_local_public_transaction_receipt(
                transaction_hash,
                &transaction,
                7,
                &inclusion,
                &[discontinuous_confirmation],
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_an_account_list_that_exceeds_the_response_bound() {
        let transaction = public_transaction(
            vec![AccountId::new([3; 32]); MAX_LOCAL_PUBLIC_TRANSACTION_RECEIPT_ACCOUNTS + 1],
            vec![],
        );
        let transaction_hash = HashType(transaction.hash());
        let inclusion = produce_dummy_block(
            7,
            Some(HashType([1; 32])),
            vec![common::transaction::LeeTransaction::Public(
                transaction.clone(),
            )],
        );

        assert!(
            build_local_public_transaction_receipt(
                transaction_hash,
                &transaction,
                7,
                &inclusion,
                &[],
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_confirmation_depth_above_the_response_bound() {
        assert!(validate_confirmation_depth(33).is_err());
        assert!(validate_confirmation_depth(32).is_ok());
    }
}
