use std::collections::BTreeMap;

use jsonrpsee::proc_macros::rpc;
#[cfg(feature = "server")]
use jsonrpsee::types::ErrorObjectOwned;
#[cfg(feature = "client")]
pub use jsonrpsee::{
    core::{ClientError, http_helpers::HttpError},
    http_client::HttpClientBuilder as SequencerClientBuilder,
};
use sequencer_service_protocol::{
    Account, AccountId, Block, BlockId, ChannelId, Commitment, CommitmentSetDigest,
    CrossZoneDeadLetterReport, HashType, LeeTransaction, LocalPublicBlockHistoryPageV1,
    LocalPublicBlockHistoryRequestV1, LocalPublicTransactionReceiptV1, MembershipProof, Nonce,
    ProgramId,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

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
/// let url = "http://localhost:3040".parse()?;
/// let client = SequencerClientBuilder::default().build(url)?;
///
/// let tx: LeeTransaction = unimplemented!("Construct your transaction here");
/// let tx_hash = client.send_transaction(tx).await?;
/// ```
#[cfg(feature = "client")]
pub type SequencerClient = jsonrpsee::http_client::HttpClient;

/// Result returned by `getTransaction`.
///
/// Canonical responses contain one transaction. Older sequencers return a
/// `(transaction, block_id)` tuple; both forms remain accepted by this client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionLookupResponse(pub Option<LeeTransaction>);

impl Serialize for TransactionLookupResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for TransactionLookupResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct TransactionLookupVisitor;

        impl<'de> serde::de::Visitor<'de> for TransactionLookupVisitor {
            type Value = TransactionLookupResponse;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(
                    "null, a serialized transaction string, or a two-element transaction tuple",
                )
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(TransactionLookupResponse(None))
            }

            fn visit_none<E>(self) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(TransactionLookupResponse(None))
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                LeeTransaction::deserialize(serde::de::value::StrDeserializer::<E>::new(v))
                    .map(|transaction| TransactionLookupResponse(Some(transaction)))
                    .map_err(E::custom)
            }

            fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                self.visit_str(&v)
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let transaction = seq
                    .next_element::<LeeTransaction>()?
                    .ok_or_else(|| A::Error::custom("transaction tuple is missing payload"))?;
                let _block_id = seq
                    .next_element::<u64>()?
                    .ok_or_else(|| A::Error::custom("transaction tuple is missing block id"))?;
                if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                    return Err(A::Error::custom(
                        "transaction tuple has more than two elements",
                    ));
                }
                Ok(TransactionLookupResponse(Some(transaction)))
            }
        }

        deserializer.deserialize_any(TransactionLookupVisitor)
    }
}

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

    #[method(name = "getTransaction")]
    async fn get_transaction(
        &self,
        tx_hash: HashType,
    ) -> Result<TransactionLookupResponse, ErrorObjectOwned>;

    /// Returns a bounded receipt from the local sequencer for a public transaction.
    ///
    /// This is local-chain data, not an independently verifiable inclusion proof or
    /// Bedrock finality.
    #[method(name = "getLocalPublicTransactionReceipt")]
    async fn get_local_public_transaction_receipt(
        &self,
        tx_hash: HashType,
        confirmation_depth: u8,
    ) -> Result<Option<LocalPublicTransactionReceiptV1>, ErrorObjectOwned>;

    /// Returns a bounded, snapshot-bound page of trusted local public block history.
    #[method(name = "getLocalPublicBlockHistory")]
    async fn get_local_public_block_history(
        &self,
        request: LocalPublicBlockHistoryRequestV1,
    ) -> Result<LocalPublicBlockHistoryPageV1, ErrorObjectOwned>;

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

    /// Lists every program currently deployed in sequencer state, ordered by identifier.
    #[method(name = "listPrograms")]
    async fn list_programs(&self) -> Result<Vec<ProgramId>, ErrorObjectOwned>;

    #[method(name = "getChannelId")]
    async fn get_channel_id(&self) -> Result<ChannelId, ErrorObjectOwned>;

    /// The cross-zone deliveries this sequencer has given up on.
    ///
    /// Its own method rather than folded into `checkHealth`: one undeliverable
    /// peer message must not read as an unhealthy node.
    #[method(name = "getCrossZoneDeadLetters")]
    async fn get_cross_zone_dead_letters(
        &self,
    ) -> Result<CrossZoneDeadLetterReport, ErrorObjectOwned>;
}
