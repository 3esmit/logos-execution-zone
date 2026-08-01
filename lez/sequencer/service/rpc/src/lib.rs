use std::collections::BTreeMap;

use jsonrpsee::proc_macros::rpc;
#[cfg(feature = "server")]
use jsonrpsee::types::ErrorObjectOwned;
#[cfg(feature = "client")]
pub use jsonrpsee::{core::ClientError, http_client::HttpClientBuilder as SequencerClientBuilder};
use sequencer_service_protocol::{
    Account, AccountId, Block, BlockId, ChannelId, Commitment, CommitmentSetDigest, HashType,
    LeeTransaction, LocalPublicTransactionReceiptV1, MembershipProof, Nonce, ProgramId,
};

#[cfg(all(not(feature = "server"), not(feature = "client")))]
compile_error!("At least one of `server` or `client` features must be enabled.");

/// Type alias for RPC client. Only available when `client` feature is enabled.
///
/// It's cheap to clone this client, so it can be cloned and shared across the application.
///
/// # Example
///
/// ```no_run
/// use common::transaction::LeeTransaction;
/// use sequencer_service_rpc::{RpcClient as _, SequencerClientBuilder};
///
/// async fn example() -> Result<(), Box<dyn std::error::Error>> {
///     let url = "http://localhost:3040";
///     let client = SequencerClientBuilder::default().build(url)?;
///
///     let tx: LeeTransaction = unimplemented!("Construct your transaction here");
///     let _tx_hash = client.send_transaction(tx).await?;
///     Ok(())
/// }
/// ```
#[cfg(feature = "client")]
pub type SequencerClient = jsonrpsee::http_client::HttpClient;

#[cfg_attr(all(feature = "server", not(feature = "client")), rpc(server))]
#[cfg_attr(all(feature = "client", not(feature = "server")), rpc(client))]
#[cfg_attr(all(feature = "server", feature = "client"), rpc(server, client))]
pub trait Rpc {
    #[method(name = "sendTransaction")]
    async fn send_transaction(&self, tx: LeeTransaction) -> Result<HashType, ErrorObjectOwned>;

    // TODO: expand healthcheck response into some kind of report
    #[method(name = "checkHealth")]
    async fn check_health(&self) -> Result<(), ErrorObjectOwned>;

    // TODO: These functions should be removed after wallet starts using indexer
    // for this type of queries.
    //
    // =============================================================================================

    #[method(name = "getBlock")]
    async fn get_block(&self, block_id: BlockId) -> Result<Option<Block>, ErrorObjectOwned>;

    #[method(name = "getBlockRange")]
    async fn get_block_range(
        &self,
        start_block_id: BlockId,
        end_block_id: BlockId,
    ) -> Result<Vec<Block>, ErrorObjectOwned>;

    #[method(name = "getLastBlockId")]
    async fn get_last_block_id(&self) -> Result<BlockId, ErrorObjectOwned>;

    #[method(name = "getAccountBalance")]
    async fn get_account_balance(&self, account_id: AccountId) -> Result<u128, ErrorObjectOwned>;

    /// Returns the committed transaction payload. Block location is not part of the wire result.
    #[method(name = "getTransaction")]
    async fn get_transaction(
        &self,
        tx_hash: HashType,
    ) -> Result<Option<LeeTransaction>, ErrorObjectOwned>;

    /// Returns a bounded receipt from the local sequencer for a public transaction.
    ///
    /// The response is absent until the transaction is included and the requested number of
    /// successor blocks exists. This endpoint is a local-chain receipt; it is not an
    /// independently verifiable cryptographic inclusion proof or Bedrock finality.
    #[method(name = "getLocalPublicTransactionReceipt")]
    async fn get_local_public_transaction_receipt(
        &self,
        tx_hash: HashType,
        confirmation_depth: u8,
    ) -> Result<Option<LocalPublicTransactionReceiptV1>, ErrorObjectOwned>;

    #[method(name = "getAccountsNonces")]
    async fn get_accounts_nonces(
        &self,
        account_ids: Vec<AccountId>,
    ) -> Result<Vec<Nonce>, ErrorObjectOwned>;

    #[method(name = "getProofsAndRoot")]
    async fn get_proofs_and_root(
        &self,
        commitments: Vec<Commitment>,
    ) -> Result<(Vec<Option<MembershipProof>>, CommitmentSetDigest), ErrorObjectOwned>;

    #[method(name = "getAccount")]
    async fn get_account(&self, account_id: AccountId) -> Result<Account, ErrorObjectOwned>;

    #[method(name = "getProgramIds")]
    async fn get_program_ids(&self) -> Result<BTreeMap<String, ProgramId>, ErrorObjectOwned>;

    /// Lists every program currently registered in sequencer state, ordered by identifier.
    #[method(name = "listPrograms")]
    async fn list_programs(&self) -> Result<Vec<ProgramId>, ErrorObjectOwned>;

    #[method(name = "getChannelId")]
    async fn get_channel_id(&self) -> Result<ChannelId, ErrorObjectOwned>;

    // =============================================================================================
}

#[cfg(test)]
mod tests {
    use sequencer_service_protocol::{
        AccountId, HashType, LocalBlockHeaderReceiptV1, LocalPublicTransactionReceiptV1,
    };

    use super::LeeTransaction;

    #[test]
    fn transaction_lookup_wire_result_is_a_single_transaction() -> Result<(), serde_json::Error> {
        let expected = common::test_utils::produce_dummy_empty_transaction();
        let wire = serde_json::to_value(&expected)?;
        assert!(wire.is_string());

        let decoded = serde_json::from_value::<Option<LeeTransaction>>(wire)?;
        assert_eq!(decoded, Some(expected));
        Ok(())
    }

    #[test]
    fn local_public_transaction_receipt_wire_shape_is_structured() -> Result<(), serde_json::Error>
    {
        let receipt = LocalPublicTransactionReceiptV1 {
            transaction_hash: HashType([1_u8; 32]),
            program_id: [2_u32; 8],
            account_ids: vec![AccountId::new([3_u8; 32])],
            instruction_word_count: 2,
            instruction_data_sha256: HashType([4_u8; 32]),
            inclusion: LocalBlockHeaderReceiptV1 {
                block_id: 7,
                block_hash: HashType([5_u8; 32]),
                previous_block_hash: HashType([6_u8; 32]),
            },
            confirmation_chain: vec![LocalBlockHeaderReceiptV1 {
                block_id: 8,
                block_hash: HashType([7_u8; 32]),
                previous_block_hash: HashType([5_u8; 32]),
            }],
        };

        let wire = serde_json::to_value(&receipt)?;
        assert_eq!(wire["transaction_hash"], "01".repeat(32));
        assert_eq!(
            wire["program_id"],
            serde_json::json!([2_u32, 2, 2, 2, 2, 2, 2, 2])
        );
        assert_eq!(
            wire["account_ids"][0],
            AccountId::new([3_u8; 32]).to_string()
        );
        assert_eq!(wire["inclusion"]["block_id"], 7);
        assert_eq!(wire["confirmation_chain"].as_array().map(Vec::len), Some(1));

        let decoded = serde_json::from_value::<LocalPublicTransactionReceiptV1>(wire)?;
        assert_eq!(decoded, receipt);
        Ok(())
    }
}
