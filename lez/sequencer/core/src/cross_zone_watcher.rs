use std::{collections::BTreeMap, time::Duration};

use common::{block::Block, transaction::LeeTransaction};
use cross_zone_inbox_core::{
    CrossZoneMessage, InboxConfig, build_inbox_dispatch_tx, inbox_config_account_id,
};
use futures::StreamExt as _;
use lee::{AccountId, program::Program};
use lee_core::{account::Account, program::ProgramId};
use log::{error, info, warn};
use logos_blockchain_core::mantle::ops::channel::ChannelId;
use logos_blockchain_zone_sdk::{
    CommonHttpClient, ZoneMessage, adapter::NodeHttpClient, indexer::ZoneIndexer,
};
use mempool::MemPoolHandle;
use ping_core::SenderInstruction;

use crate::{
    TransactionOrigin,
    config::{BedrockConfig, CrossZoneConfig},
};

/// The inbox config account this zone seeds at startup so the inbox guest can
/// authorize inbound peer messages. The config is zone-specific (self zone plus
/// per-peer target allowlists), so it cannot live in the shared genesis state.
#[must_use]
pub fn inbox_config_account(self_zone: [u8; 32], cross_zone: &CrossZoneConfig) -> (AccountId, Account) {
    let inbox_id = Program::cross_zone_inbox().id();

    let mut allowed_targets = BTreeMap::new();
    for peer in &cross_zone.peers {
        allowed_targets.insert(peer.channel_id, peer.allowed_targets.clone());
    }
    let config = InboxConfig {
        self_zone,
        allowed_peers: BTreeMap::new(),
        allowed_targets,
    };

    let account = Account {
        program_owner: inbox_id,
        balance: 0,
        data: config
            .to_bytes()
            .try_into()
            .expect("inbox config fits in account data"),
        nonce: 0_u128.into(),
    };
    (inbox_config_account_id(inbox_id), account)
}

/// Spawns one watcher task per configured peer. Each task reads the peer's
/// finalized blocks from Bedrock, recognizes outbound messages addressed to this
/// zone, and injects the matching inbox dispatch as a sequencer-origin
/// transaction into the local mempool.
pub fn spawn_watchers(
    bedrock_config: &BedrockConfig,
    cross_zone: &CrossZoneConfig,
    poll_interval: Duration,
    mempool_handle: MemPoolHandle<(TransactionOrigin, LeeTransaction)>,
) {
    let self_zone: [u8; 32] = *bedrock_config.channel_id.as_ref();
    let inbox_id = Program::cross_zone_inbox().id();
    let emitter_id = Program::ping_sender().id();

    for peer in cross_zone.peers.clone() {
        let node = NodeHttpClient::new(
            CommonHttpClient::new(bedrock_config.auth.clone().map(Into::into)),
            bedrock_config.node_url.clone(),
        );
        tokio::spawn(watch_peer(
            ZoneIndexer::new(ChannelId::from(peer.channel_id), node),
            peer.channel_id,
            peer.allowed_targets,
            self_zone,
            inbox_id,
            emitter_id,
            poll_interval,
            mempool_handle.clone(),
        ));
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Each parameter is an independent piece of per-peer watcher state"
)]
async fn watch_peer(
    zone_indexer: ZoneIndexer<NodeHttpClient>,
    peer_zone: [u8; 32],
    allowed_targets: Vec<ProgramId>,
    self_zone: [u8; 32],
    inbox_id: ProgramId,
    emitter_id: ProgramId,
    poll_interval: Duration,
    mempool_handle: MemPoolHandle<(TransactionOrigin, LeeTransaction)>,
) {
    info!("Cross-zone watcher started for peer {}", hex::encode(peer_zone));

    let mut cursor = None;
    loop {
        let stream = match zone_indexer.next_messages(cursor).await {
            Ok(stream) => stream,
            Err(err) => {
                error!(
                    "Watcher next_messages failed for peer {}: {err}",
                    hex::encode(peer_zone)
                );
                tokio::time::sleep(poll_interval).await;
                continue;
            }
        };
        let mut stream = std::pin::pin!(stream);

        while let Some((msg, slot)) = stream.next().await {
            let zone_block = match msg {
                ZoneMessage::Block(block) => block,
                ZoneMessage::Deposit(_) | ZoneMessage::Withdraw(_) => continue,
            };
            match borsh::from_slice::<Block>(&zone_block.data) {
                Ok(block) => {
                    deliver_block(
                        &block,
                        peer_zone,
                        self_zone,
                        inbox_id,
                        emitter_id,
                        &allowed_targets,
                        &mempool_handle,
                    )
                    .await;
                }
                Err(err) => error!("Watcher failed to deserialize peer block: {err}"),
            }
            cursor = Some(slot);
        }

        // Stream ended (caught up to the peer's last finalized block); poll again.
        tokio::time::sleep(poll_interval).await;
    }
}

/// Scans one peer block for outbound messages and injects a dispatch per match.
///
/// Option A (M3): the watcher recognizes the demo emitter and reads the outbound
/// message straight off its instruction. M4 replaces this with re-derivation
/// from the outbox PDA write, which removes the emitter-specific decoding.
async fn deliver_block(
    block: &Block,
    peer_zone: [u8; 32],
    self_zone: [u8; 32],
    inbox_id: ProgramId,
    emitter_id: ProgramId,
    allowed_targets: &[ProgramId],
    mempool_handle: &MemPoolHandle<(TransactionOrigin, LeeTransaction)>,
) {
    for (index, tx) in block.body.transactions.iter().enumerate() {
        let LeeTransaction::Public(public_tx) = tx else {
            continue;
        };
        let message = public_tx.message();
        if message.program_id != emitter_id {
            continue;
        }

        let SenderInstruction::Send {
            target_zone,
            target_program_id,
            target_accounts,
            payload,
            ..
        } = match risc0_zkvm::serde::from_slice(&message.instruction_data) {
            Ok(send) => send,
            Err(err) => {
                warn!("Watcher could not decode emitter instruction: {err}");
                continue;
            }
        };

        if target_zone != self_zone {
            continue;
        }
        if !allowed_targets.contains(&target_program_id) {
            warn!(
                "Watcher dropping message to disallowed target from peer {}",
                hex::encode(peer_zone)
            );
            continue;
        }

        let cross_zone_message = CrossZoneMessage {
            src_zone: peer_zone,
            src_block_id: block.header.block_id,
            src_tx_index: u32::try_from(index).unwrap_or(u32::MAX),
            src_program_id: emitter_id,
            target_program_id,
            payload,
            l1_inclusion_witness: None,
        };
        let target_ids: Vec<AccountId> = target_accounts.into_iter().map(AccountId::new).collect();
        let dispatch = build_inbox_dispatch_tx(inbox_id, &cross_zone_message, target_ids);

        match mempool_handle
            .push((TransactionOrigin::Sequencer, LeeTransaction::Public(dispatch)))
            .await
        {
            Ok(()) => info!(
                "Watcher injected cross-zone dispatch from peer {} block {} tx {}",
                hex::encode(peer_zone),
                block.header.block_id,
                index
            ),
            Err(err) => error!("Watcher failed to enqueue inbox dispatch: {err}"),
        }
    }
}
