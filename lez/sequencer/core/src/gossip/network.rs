//! The libp2p swarm and its drive task.
//!
//! The drive task owns the [`Swarm`]; [`Libp2pNetwork`] is the handle. A
//! drive-task failure after startup is log-and-continue: the node keeps
//! running L1-only (see the gossip spec).

use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use anyhow::{Context as _, Result, anyhow};
use common::transaction::LeeTransaction;
use futures::StreamExt as _;
#[cfg(feature = "mdns")]
use libp2p::mdns;
use libp2p::{
    Multiaddr, PeerId, SwarmBuilder, gossipsub, identify,
    identity::Keypair,
    kad,
    multiaddr::Protocol,
    swarm::{NetworkBehaviour, Swarm, SwarmEvent},
};
use log::{debug, error, info, warn};
use mempool::MemPoolHandle;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use crate::{TransactionOrigin, config::GossipConfig, gossip::seen_cache::SeenCache};

/// How long to wait for the first listen address before failing startup.
const LISTEN_TIMEOUT: Duration = Duration::from_secs(5);
/// Cadence of the post-death reminder that the node is running L1-only.
const DEATH_REMINDER_INTERVAL: Duration = Duration::from_secs(300);
/// Recently-seen gossiped transaction hashes kept for dedup.
const SEEN_CACHE_CAPACITY: usize = 4096;
/// Outbound local-publish channel depth; `try_send` drops on overflow.
const TX_PUBLISH_CHANNEL_CAPACITY: usize = 1024;

#[derive(NetworkBehaviour)]
struct GossipBehaviour {
    gossipsub: gossipsub::Behaviour,
    identify: identify::Behaviour,
    kademlia: kad::Behaviour<kad::store::MemoryStore>,
    #[cfg(feature = "mdns")]
    mdns: mdns::tokio::Behaviour,
}

/// The seam `SequencerCore` and tests see. Later phases extend it with
/// publish operations and inbound sinks.
pub trait PeerNetworkTrait {
    /// Ed25519 public keys of currently connected peers.
    fn connected_peers(&self) -> Vec<[u8; 32]>;

    /// Cancelled when the drive task terminates. Unlike the publisher's
    /// token, observers must NOT halt the node on it — gossip is an
    /// optimization.
    fn driver_cancellation(&self) -> CancellationToken;
}

/// Handle to the running gossip network. Dropping it aborts the drive task.
pub struct Libp2pNetwork {
    connected_rx: watch::Receiver<Vec<[u8; 32]>>,
    driver_cancellation: CancellationToken,
    driver: tokio::task::JoinHandle<()>,
    listen_addrs: Vec<Multiaddr>,
    local_peer_id: PeerId,
    tx_tx: mpsc::Sender<LeeTransaction>,
}

/// Handle for publishing locally-submitted transactions to the gossip mesh.
/// `publish` is non-blocking: a full channel drops the transaction rather
/// than back-pressuring the caller.
#[derive(Clone)]
pub struct TxPublisher(mpsc::Sender<LeeTransaction>);

impl TxPublisher {
    pub fn publish(&self, tx: LeeTransaction) {
        if let Err(err) = self.0.try_send(tx) {
            debug!("Dropping local tx publish: outbound gossip channel full or closed: {err}");
        }
    }
}

impl Libp2pNetwork {
    /// Builds the swarm, binds `listen_addr`, seeds Kademlia and dials
    /// bootstrap peers, and spawns the drive task. Fails fast on a bad
    /// listen/bootstrap multiaddr or bind failure; after that, gossip errors
    /// never halt the node.
    pub async fn start(
        config: GossipConfig,
        channel_id: [u8; 32],
        secret_key: [u8; 32],
        mempool_handle: MemPoolHandle<(TransactionOrigin, LeeTransaction)>,
        max_block_size: u64,
    ) -> Result<Self> {
        let mut secret_for_libp2p = secret_key;
        let keypair = Keypair::ed25519_from_bytes(&mut secret_for_libp2p)
            .map_err(|err| anyhow!("Invalid bedrock signing key for libp2p identity: {err}"))?;
        let local_peer_id = keypair.public().to_peer_id();

        let listen_addr: Multiaddr = config
            .listen_addr
            .parse()
            .with_context(|| format!("Invalid gossip listen_addr `{}`", config.listen_addr))?;
        let bootstrap: Vec<Multiaddr> = config
            .bootstrap_peers
            .iter()
            .map(|addr| {
                addr.parse()
                    .with_context(|| format!("Invalid gossip bootstrap peer `{addr}`"))
            })
            .collect::<Result<_>>()?;

        let topic = gossipsub::IdentTopic::new(format!("/lez/{}/v1/txs", hex::encode(channel_id)));

        let message_id_fn = |msg: &gossipsub::Message| {
            let id = borsh::from_slice::<LeeTransaction>(&msg.data)
                .map_or_else(|_| msg.data.clone(), |tx| tx.hash().0.to_vec());
            gossipsub::MessageId::from(id)
        };
        let gossipsub_config = gossipsub::ConfigBuilder::default()
            .validation_mode(gossipsub::ValidationMode::Strict)
            .message_id_fn(message_id_fn)
            .validate_messages()
            .build()
            .map_err(|err| anyhow!("Failed to build gossipsub config: {err}"))?;
        let gossipsub_behaviour = gossipsub::Behaviour::new(
            gossipsub::MessageAuthenticity::Signed(keypair.clone()),
            gossipsub_config,
        )
        .map_err(|err| anyhow!("Failed to build gossipsub behaviour: {err}"))?;
        let identify_behaviour =
            identify::Behaviour::new(identify::Config::new("/lez/1".to_owned(), keypair.public()));
        let kademlia_behaviour = {
            let store = kad::store::MemoryStore::new(local_peer_id);
            let mut kademlia = kad::Behaviour::new(local_peer_id, store);
            kademlia.set_mode(Some(kad::Mode::Server));
            kademlia
        };
        #[cfg(feature = "mdns")]
        let mdns_behaviour = mdns::tokio::Behaviour::new(mdns::Config::default(), local_peer_id)
            .map_err(|err| anyhow!("Failed to build mdns behaviour: {err}"))?;

        let mut swarm = SwarmBuilder::with_existing_identity(keypair)
            .with_tokio()
            .with_quic()
            .with_behaviour(|_key| GossipBehaviour {
                gossipsub: gossipsub_behaviour,
                identify: identify_behaviour,
                kademlia: kademlia_behaviour,
                #[cfg(feature = "mdns")]
                mdns: mdns_behaviour,
            })
            .expect("behaviour constructor is infallible")
            .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(Duration::from_secs(60)))
            .build();

        swarm
            .behaviour_mut()
            .gossipsub
            .subscribe(&topic)
            .context("Failed to subscribe to gossip tx topic")?;

        swarm
            .listen_on(listen_addr)
            .context("Failed to listen on gossip address")?;

        // Fail fast on bind errors: wait for the first listen address.
        let listen_addrs = wait_for_listen_addr(&mut swarm).await?;
        info!("Gossip listening on {listen_addrs:?} as {local_peer_id}");

        // Seed Kademlia with bootstrap peers that carry an embedded peer id;
        // dial the rest directly, since Kademlia can't route to an address
        // without a known peer id.
        for addr in &bootstrap {
            let embedded_peer_id = match addr.iter().last() {
                Some(Protocol::P2p(peer_id)) => Some(peer_id),
                _ => None,
            };
            if let Some(peer_id) = embedded_peer_id {
                swarm
                    .behaviour_mut()
                    .kademlia
                    .add_address(&peer_id, addr.clone());
                continue;
            }
            if let Err(err) = swarm.dial(addr.clone()) {
                warn!("Failed to dial gossip bootstrap peer {addr}: {err}");
            }
        }
        if let Err(err) = swarm.behaviour_mut().kademlia.bootstrap() {
            debug!("Kademlia bootstrap skipped (no known peers yet): {err}");
        }

        let (connected_tx, connected_rx) = watch::channel(Vec::new());
        let driver_cancellation = CancellationToken::new();
        let (tx_tx, tx_rx) = mpsc::channel::<LeeTransaction>(TX_PUBLISH_CHANNEL_CAPACITY);

        let driver = tokio::spawn(run_drive_task(DriveTask {
            swarm,
            connected: HashSet::new(),
            pubkeys: HashMap::new(),
            connected_tx,
            cancellation: driver_cancellation.clone(),
            topic,
            mempool: mempool_handle,
            seen: SeenCache::new(SEEN_CACHE_CAPACITY),
            max_block_size,
            tx_rx,
        }));
        spawn_death_reminder(driver_cancellation.clone());

        Ok(Self {
            connected_rx,
            driver_cancellation,
            driver,
            listen_addrs,
            local_peer_id,
            tx_tx,
        })
    }

    #[must_use]
    pub fn listen_addrs(&self) -> Vec<Multiaddr> {
        self.listen_addrs.clone()
    }

    #[must_use]
    pub const fn local_peer_id(&self) -> PeerId {
        self.local_peer_id
    }

    /// Handle for publishing locally-submitted transactions to the mesh.
    #[must_use]
    pub fn tx_publisher(&self) -> TxPublisher {
        TxPublisher(self.tx_tx.clone())
    }
}

impl PeerNetworkTrait for Libp2pNetwork {
    fn connected_peers(&self) -> Vec<[u8; 32]> {
        self.connected_rx.borrow().clone()
    }

    fn driver_cancellation(&self) -> CancellationToken {
        self.driver_cancellation.clone()
    }
}

impl Drop for Libp2pNetwork {
    fn drop(&mut self) {
        self.driver.abort();
        self.driver_cancellation.cancel();
    }
}

/// Everything the drive task owns.
struct DriveTask {
    swarm: Swarm<GossipBehaviour>,
    connected: HashSet<PeerId>,
    /// Ed25519 public keys of peers seen via Identify, keyed by `PeerId`.
    pubkeys: HashMap<PeerId, [u8; 32]>,
    connected_tx: watch::Sender<Vec<[u8; 32]>>,
    cancellation: CancellationToken,
    topic: gossipsub::IdentTopic,
    mempool: MemPoolHandle<(TransactionOrigin, LeeTransaction)>,
    seen: SeenCache,
    max_block_size: u64,
    tx_rx: mpsc::Receiver<LeeTransaction>,
}

impl DriveTask {
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "SwarmEvent is non_exhaustive; only connection and behaviour events are handled"
    )]
    fn on_swarm_event(&mut self, event: SwarmEvent<GossipBehaviourEvent>) {
        match event {
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                self.connected.insert(peer_id);
                self.update_connected_watch();
            }
            SwarmEvent::ConnectionClosed {
                peer_id,
                num_established: 0,
                ..
            } => {
                self.connected.remove(&peer_id);
                self.pubkeys.remove(&peer_id);
                self.update_connected_watch();
            }
            SwarmEvent::Behaviour(behaviour_event) => self.on_behaviour_event(behaviour_event),
            _ => {}
        }
    }

    // `GossipBehaviourEvent` is generated by `#[derive(NetworkBehaviour)]`;
    // clippy does not flag wildcard matches against macro-generated enums,
    // so no `#[expect(clippy::wildcard_enum_match_arm)]` is needed here.
    fn on_behaviour_event(&mut self, event: GossipBehaviourEvent) {
        match event {
            GossipBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                propagation_source,
                message_id,
                message,
            }) => {
                self.on_gossip_message(propagation_source, &message_id, &message.data);
            }
            GossipBehaviourEvent::Identify(identify::Event::Received { peer_id, info, .. }) => {
                if let Ok(ed25519_pubkey) = info.public_key.try_into_ed25519() {
                    self.pubkeys.insert(peer_id, ed25519_pubkey.to_bytes());
                    self.update_connected_watch();
                }
                for addr in info.listen_addrs {
                    self.swarm
                        .behaviour_mut()
                        .kademlia
                        .add_address(&peer_id, addr);
                }
            }
            #[cfg(feature = "mdns")]
            GossipBehaviourEvent::Mdns(mdns::Event::Discovered(peers)) => {
                for (peer_id, addr) in peers {
                    if let Err(err) = self.swarm.dial(addr) {
                        debug!("Failed to dial mdns-discovered peer {peer_id}: {err}");
                    }
                }
            }
            _ => {}
        }
    }

    fn update_connected_watch(&self) {
        let mut peers: Vec<[u8; 32]> = self
            .connected
            .iter()
            .filter_map(|peer_id| self.pubkeys.get(peer_id).copied())
            .collect();
        peers.sort_unstable();
        self.connected_tx.send_if_modified(|current| {
            if *current == peers {
                false
            } else {
                *current = peers;
                true
            }
        });
    }

    /// Validates an inbound gossiped transaction and reports the mesh
    /// acceptance decision, admitting it to the mempool on first sight.
    fn on_gossip_message(
        &mut self,
        source: PeerId,
        message_id: &gossipsub::MessageId,
        data: &[u8],
    ) {
        use crate::gossip::validation::{TxEvaluation, evaluate_transaction};

        let acceptance = match evaluate_transaction(data, self.max_block_size) {
            TxEvaluation::Reject(reason) => {
                debug!("Rejecting gossiped tx from {source}: {reason}");
                gossipsub::MessageAcceptance::Reject
            }
            TxEvaluation::Ignore => gossipsub::MessageAcceptance::Ignore,
            TxEvaluation::Accept(tx) => {
                let hash = tx.hash();
                if !self.seen.insert(hash) {
                    gossipsub::MessageAcceptance::Ignore
                } else if self
                    .mempool
                    .try_push((TransactionOrigin::Gossip, tx))
                    .is_err()
                {
                    debug!("Mempool full; dropping gossiped tx {hash:?}");
                    gossipsub::MessageAcceptance::Ignore
                } else {
                    gossipsub::MessageAcceptance::Accept
                }
            }
        };
        _ = self
            .swarm
            .behaviour_mut()
            .gossipsub
            .report_message_validation_result(message_id, &source, acceptance);
    }

    /// Publishes a locally-submitted transaction to the mesh.
    fn publish_transaction(&mut self, tx: &LeeTransaction) {
        let hash = tx.hash();
        self.seen.insert(hash);
        let bytes = borsh::to_vec(tx).expect("tx borsh serialization should not fail");
        if let Err(err) = self
            .swarm
            .behaviour_mut()
            .gossipsub
            .publish(self.topic.clone(), bytes)
        {
            debug!("Skipping local tx publish {hash:?}: {err}");
        }
    }
}

/// Derives the libp2p `PeerId` an Ed25519 public key produces. `None` only
/// for byte strings that are not a valid curve point.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "unused by the mesh until a later gossip task; exercised by the identity test below"
    )
)]
pub(crate) fn peer_id_from_ed25519(pubkey: &[u8; 32]) -> Option<PeerId> {
    libp2p::identity::ed25519::PublicKey::try_from_bytes(pubkey)
        .ok()
        .map(|key| libp2p::identity::PublicKey::from(key).to_peer_id())
}

#[expect(
    clippy::integer_division_remainder_used,
    reason = "Generated by select! macro, can't be easily rewritten to avoid this lint"
)]
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "SwarmEvent is non_exhaustive; only startup listener events are handled here"
)]
async fn wait_for_listen_addr(swarm: &mut Swarm<GossipBehaviour>) -> Result<Vec<Multiaddr>> {
    let deadline = tokio::time::sleep(LISTEN_TIMEOUT);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            event = swarm.select_next_some() => match event {
                SwarmEvent::NewListenAddr { address, .. } => return Ok(vec![address]),
                SwarmEvent::ListenerError { error, .. } => {
                    return Err(anyhow!("Gossip listener error during startup: {error}"));
                }
                SwarmEvent::ListenerClosed { reason, .. } => {
                    return Err(anyhow!("Gossip listener closed during startup: {reason:?}"));
                }
                _ => {}
            },
            () = &mut deadline => {
                return Err(anyhow!("Timed out waiting for gossip listen address"));
            }
        }
    }
}

#[expect(
    clippy::integer_division_remainder_used,
    reason = "Generated by select! macro, can't be easily rewritten to avoid this lint"
)]
async fn run_drive_task(mut task: DriveTask) {
    // Cancelled on return or panic, so observers learn the driver is gone.
    let _guard = task.cancellation.clone().drop_guard();

    loop {
        tokio::select! {
            event = task.swarm.select_next_some() => task.on_swarm_event(event),
            Some(tx) = task.tx_rx.recv() => task.publish_transaction(&tx),
        }
    }
}

/// After the driver dies, remind operators the node is running L1-only.
fn spawn_death_reminder(cancellation: CancellationToken) {
    tokio::spawn(async move {
        cancellation.cancelled().await;
        loop {
            error!(
                "Sequencer gossip network is down; continuing L1-only. \
                 Restart the node to restore p2p."
            );
            tokio::time::sleep(DEATH_REMINDER_INTERVAL).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use logos_blockchain_key_management_system_service::keys::Ed25519Key;

    use super::*;
    use crate::config::GossipConfig;

    const TEST_MAX_BLOCK_SIZE: u64 = 1 << 20;

    fn test_config() -> GossipConfig {
        GossipConfig {
            listen_addr: "/ip4/127.0.0.1/udp/0/quic-v1".to_owned(),
            bootstrap_peers: vec![],
        }
    }

    fn test_mempool_handle() -> MemPoolHandle<(TransactionOrigin, LeeTransaction)> {
        mempool::MemPool::new(1000).1
    }

    #[test]
    fn libp2p_identity_matches_kms_public_key() {
        // The PeerId derived from an Ed25519 public key must equal the
        // PeerId the same secret produces as a libp2p identity.
        let secret = [9; 32];
        let kms_pubkey = Ed25519Key::from_bytes(&secret).public_key().to_bytes();
        let mut secret_for_libp2p = secret;
        let keypair =
            libp2p::identity::Keypair::ed25519_from_bytes(&mut secret_for_libp2p).unwrap();
        assert_eq!(
            peer_id_from_ed25519(&kms_pubkey).unwrap(),
            keypair.public().to_peer_id()
        );
    }

    #[tokio::test]
    async fn start_binds_and_reports_listen_addr() {
        let network = Libp2pNetwork::start(
            test_config(),
            [1; 32],
            [9; 32],
            test_mempool_handle(),
            TEST_MAX_BLOCK_SIZE,
        )
        .await
        .unwrap();
        let addrs = network.listen_addrs();
        assert!(!addrs.is_empty());
        assert!(addrs[0].to_string().contains("/udp/"));
        assert!(network.connected_peers().is_empty());
    }

    #[tokio::test]
    async fn start_fails_fast_on_bad_listen_addr() {
        let config = GossipConfig {
            listen_addr: "not a multiaddr".to_owned(),
            ..test_config()
        };
        assert!(
            Libp2pNetwork::start(
                config,
                [1; 32],
                [9; 32],
                test_mempool_handle(),
                TEST_MAX_BLOCK_SIZE
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn start_fails_fast_on_bad_bootstrap_addr() {
        let config = GossipConfig {
            bootstrap_peers: vec!["nonsense".to_owned()],
            ..test_config()
        };
        assert!(
            Libp2pNetwork::start(
                config,
                [1; 32],
                [9; 32],
                test_mempool_handle(),
                TEST_MAX_BLOCK_SIZE
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn drop_cancels_driver() {
        let network = Libp2pNetwork::start(
            test_config(),
            [1; 32],
            [9; 32],
            test_mempool_handle(),
            TEST_MAX_BLOCK_SIZE,
        )
        .await
        .unwrap();
        let token = network.driver_cancellation();
        drop(network);
        tokio::time::timeout(std::time::Duration::from_secs(5), token.cancelled())
            .await
            .expect("driver should stop when the handle is dropped");
    }
}
