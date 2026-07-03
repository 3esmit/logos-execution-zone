use std::{path::Path, sync::Arc};

use anyhow::Result;
use arc_swap::ArcSwap;
use common::block::Block;
// ToDo: Remove after testnet
use futures::StreamExt as _;
use log::{error, info, warn};
use logos_blockchain_core::header::HeaderId;
use logos_blockchain_zone_sdk::{
    CommonHttpClient, ZoneMessage, adapter::NodeHttpClient, indexer::ZoneIndexer,
};

use crate::{
    block_store::IndexerStore,
    config::IndexerConfig,
    cross_zone_verifier::CrossZoneVerifier,
    status::{IndexerStatus, IndexerSyncStatus},
};

pub mod block_store;
pub mod config;
pub mod cross_zone_verifier;
pub mod status;

#[derive(Clone)]
pub struct IndexerCore {
    pub zone_indexer: Arc<ZoneIndexer<NodeHttpClient>>,
    pub config: IndexerConfig,
    pub store: IndexerStore,
    /// Live ingestion status; updated by the ingest stream, read by `status`.
    pub status: Arc<ArcSwap<IndexerSyncStatus>>,
    /// Option B cross-zone verifier; `None` when cross-zone messaging is disabled.
    pub verifier: Option<CrossZoneVerifier>,
}

impl IndexerCore {
    pub fn new(config: IndexerConfig, storage_dir: &Path) -> Result<Self> {
        // Namespace the DB by channel so indexers on different channels can
        // share a storage dir without their RocksDB state colliding.
        let home = storage_dir.join(format!("rocksdb-{}", config.channel_id));

        let basic_auth = config.bedrock_config.auth.clone().map(Into::into);
        let node = NodeHttpClient::new(
            CommonHttpClient::new(basic_auth),
            config.bedrock_config.addr.clone(),
        );
        let zone_indexer = ZoneIndexer::new(config.channel_id, node);

        // Genesis accounts the indexer must seed to match the sequencer's state,
        // since none are produced by a transaction: the cross-zone inbox config
        // and any bridge-lock holdings. Both go through the same builders the
        // sequencer uses, so the states are byte-identical.
        let mut genesis_seed = Vec::new();
        if let Some(cross_zone) = config.cross_zone.as_ref() {
            let self_zone: [u8; 32] = *config.channel_id.as_ref();
            genesis_seed.push(cross_zone::build_inbox_config_account(
                self_zone, cross_zone,
            ));
        }
        for holding in &config.bridge_lock_holdings {
            genesis_seed.push(cross_zone::build_holding_account(
                holding.holder,
                holding.amount,
            ));
        }

        // Option B verifier: re-derives each cross-zone dispatch from the peer's
        // finalized blocks. `None` when cross-zone messaging is disabled.
        let verifier = CrossZoneVerifier::start(&config);

        Ok(Self {
            zone_indexer: Arc::new(zone_indexer),
            store: IndexerStore::open_db(&home, genesis_seed)?,
            config,
            status: Arc::new(ArcSwap::from_pointee(IndexerSyncStatus::starting())),
            verifier,
        })
    }

    /// Snapshot of the current ingestion status (sync state + indexed tip).
    ///
    /// Combines the ingest loop's live status with the L2 tip read fresh from the
    /// store, so callers (FFI/RPC) can tell "catching up" from "failed".
    #[must_use]
    pub fn status(&self) -> IndexerStatus {
        let sync = IndexerSyncStatus::clone(&self.status.load());
        let indexed_block_id = self.store.get_last_block_id().ok().flatten();
        IndexerStatus {
            sync,
            indexed_block_id,
        }
    }

    /// Atomically publish a new ingestion status for readers of `status`.
    fn set_status(&self, status: IndexerSyncStatus) {
        self.status.store(Arc::new(status));
    }

    pub fn subscribe_parse_block_stream(&self) -> impl futures::Stream<Item = Result<Block>> + '_ {
        let poll_interval = self.config.consensus_info_polling_interval;
        let initial_cursor = self
            .store
            .get_zone_cursor()
            .expect("Failed to load zone-sdk indexer cursor");

        async_stream::stream! {
            let mut cursor = initial_cursor;

            if cursor.is_some() {
                info!("Resuming indexer from cursor {cursor:?}");
            } else {
                info!("Starting indexer from beginning of channel");
            }

            loop {
                let stream = match self.zone_indexer.next_messages(cursor).await {
                    Ok(s) => s,
                    Err(err) => {
                        // `next_messages` reads L1 consensus info internally, so
                        // this also covers an unreachable/misconfigured L1 node.
                        error!("Failed to start zone-sdk next_messages stream: {err}");
                        self.set_status(IndexerSyncStatus::error(format!(
                            "cannot reach L1 / read channel: {err}"
                        )));
                        tokio::time::sleep(poll_interval).await;
                        continue;
                    }
                };
                let mut stream = std::pin::pin!(stream);

                // Flip to Syncing on the first message of this cycle (not merely on
                // a successful poll) so the steady-state CaughtUp status doesn't
                // flicker. Until then the state stays Starting (cold-start scan of
                // empty L1 history) or CaughtUp (idle).
                let mut announced_syncing = false;

                while let Some((msg, slot)) = stream.next().await {
                    if !announced_syncing {
                        self.set_status(IndexerSyncStatus::syncing());
                        announced_syncing = true;
                    }

                    let zone_block = match msg {
                        ZoneMessage::Block(b) => b,
                        // Non-block messages don't carry a cursor position; the
                        // next ZoneBlock advances past them implicitly.
                        ZoneMessage::Deposit(_) | ZoneMessage::Withdraw(_) => continue,
                    };

                    let block: Block = match borsh::from_slice(&zone_block.data) {
                        Ok(b) => b,
                        Err(e) => {
                            error!("Failed to deserialize L2 block from zone-sdk: {e}");
                            // Advance past the broken inscription so we don't
                            // re-process it on restart.
                            cursor = Some(slot);
                            if let Err(err) = self.store.set_zone_cursor(&slot) {
                                warn!("Failed to persist indexer cursor: {err:#}");
                            }
                            continue;
                        }
                    };

                    info!("Indexed L2 block {}", block.header.block_id);

                    // Option B: re-derive and verify every cross-zone dispatch
                    // before applying the block. A forged or replayed dispatch
                    // halts ingestion rather than persisting an invalid state.
                    if let Some(verifier) = &self.verifier
                        && let Err(err) = verifier.verify_block(&block).await
                    {
                        error!(
                            "Cross-zone verification failed for block {}: {err:#}. Halting indexer ingestion.",
                            block.header.block_id
                        );
                        return;
                    }

                    // TODO: Remove l1_header placeholder once storage layer
                    // no longer requires it. Zone-sdk handles L1 tracking internally.
                    let placeholder_l1_header = HeaderId::from([0_u8; 32]);
                    if let Err(err) = self.store.put_block(block.clone(), placeholder_l1_header).await {
                        // Do not advance the cursor past a block we failed to
                        // apply: halt ingestion instead of silently desyncing.
                        error!(
                            "Failed to store block {}: {err:#}. Halting indexer ingestion.",
                            block.header.block_id
                        );
                        return;
                    }

                    cursor = Some(slot);
                    if let Err(err) = self.store.set_zone_cursor(&slot) {
                        warn!("Failed to persist indexer cursor: {err:#}");
                    }
                    yield Ok(block);
                }

                // Stream drained: caught up to LIB as of this cycle. Clears any
                // prior error (e.g. a transient L1 disconnect that left no
                // backlog, so the `Syncing` branch above never ran). Sleep then
                // poll again.
                self.set_status(IndexerSyncStatus::caught_up());
                tokio::time::sleep(poll_interval).await;
            }
        }
    }
}
