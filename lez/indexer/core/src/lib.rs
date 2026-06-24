use std::sync::Arc;

use anyhow::Result;
use common::block::Block;
// ToDo: Remove after testnet
use futures::StreamExt as _;
use log::{error, info, warn};
use logos_blockchain_core::header::HeaderId;
use logos_blockchain_zone_sdk::{
    CommonHttpClient, ZoneMessage, adapter::NodeHttpClient, indexer::ZoneIndexer,
};

use crate::{
    block_store::IndexerStore, config::IndexerConfig, cross_zone_verifier::CrossZoneVerifier,
};

pub mod block_store;
pub mod config;
pub mod cross_zone_verifier;

#[derive(Clone)]
pub struct IndexerCore {
    pub zone_indexer: Arc<ZoneIndexer<NodeHttpClient>>,
    pub config: IndexerConfig,
    pub store: IndexerStore,
    verifier: Option<CrossZoneVerifier>,
}

impl IndexerCore {
    pub fn new(config: IndexerConfig) -> Result<Self> {
        let home = config.home.join("rocksdb");

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
            genesis_seed.push(cross_zone_inbox_core::build_inbox_config_account(
                self_zone, cross_zone,
            ));
        }
        for holding in &config.bridge_lock_holdings {
            genesis_seed.push(bridge_lock_core::build_holding_account(
                holding.holder,
                holding.amount,
            ));
        }

        // Option B verifier: re-derives each cross-zone dispatch from the peer's
        // finalized blocks. `None` when cross-zone messaging is disabled.
        let verifier = CrossZoneVerifier::start(&config);

        Ok(Self {
            zone_indexer: Arc::new(zone_indexer),
            config,
            store: IndexerStore::open_db(&home, genesis_seed)?,
            verifier,
        })
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
                        error!("Failed to start zone-sdk next_messages stream: {err}");
                        tokio::time::sleep(poll_interval).await;
                        continue;
                    }
                };
                let mut stream = std::pin::pin!(stream);

                while let Some((msg, slot)) = stream.next().await {
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
                    if let Some(verifier) = &self.verifier {
                        if let Err(err) = verifier.verify_block(&block).await {
                            error!(
                                "Cross-zone verification failed for block {}: {err:#}. Halting indexer ingestion.",
                                block.header.block_id
                            );
                            return;
                        }
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

                // Stream ended (caught up to LIB). Sleep then poll again.
                tokio::time::sleep(poll_interval).await;
            }
        }
    }
}
