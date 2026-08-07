//! The libp2p swarm and its drive task.
//!
//! The drive task owns the [`Swarm`]; [`Libp2pNetwork`] is the handle. A
//! drive-task failure after startup is log-and-continue: the node keeps
//! running L1-only (see the gossip spec).

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result, anyhow};
use futures::StreamExt as _;
use libp2p::{
    Multiaddr, PeerId, SwarmBuilder, gossipsub, identify,
    identity::Keypair,
    swarm::{NetworkBehaviour, Swarm, SwarmEvent},
};
use log::{debug, error, info, warn};
use logos_blockchain_key_management_system_service::keys::Ed25519Key;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use crate::{
    config::GossipConfig,
    gossip::{
        announcement::{Announcement, MAX_LISTEN_ADDRS, announcements_topic},
        directory::PeerDirectory,
        keys_provider::AccreditedKeysProvider,
    },
};

/// How long to wait for the first listen address before failing startup.
const LISTEN_TIMEOUT: Duration = Duration::from_secs(5);
/// Sweep interval for redialing disconnected accredited peers.
const DIAL_RETRY_INTERVAL: Duration = Duration::from_secs(2);
/// Per-peer dial backoff: base * 2^attempts, capped.
const DIAL_BACKOFF_BASE: Duration = Duration::from_secs(1);
const DIAL_BACKOFF_MAX: Duration = Duration::from_secs(300);
/// Isolation warning cadence and startup grace.
const NO_PEERS_WARN_INTERVAL: Duration = Duration::from_secs(60);
const NO_PEERS_GRACE: Duration = Duration::from_secs(30);
/// Cadence of the post-death reminder that the node is running L1-only.
const DEATH_REMINDER_INTERVAL: Duration = Duration::from_secs(300);

#[derive(NetworkBehaviour)]
struct GossipBehaviour {
    gossipsub: gossipsub::Behaviour,
    identify: identify::Behaviour,
}

/// The seam `SequencerCore` and tests see. Later phases extend it with
/// publish operations and inbound sinks.
pub trait PeerNetworkTrait {
    /// Bedrock Ed25519 public keys of currently connected accredited peers.
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
}

impl Libp2pNetwork {
    /// Builds the swarm, binds `listen_addr`, subscribes to the channel's
    /// announcements topic, dials bootstrap peers, and spawns the drive
    /// task. Fails fast on a bad listen/bootstrap multiaddr or bind
    /// failure; after that, gossip errors never halt the node.
    pub async fn start<P: AccreditedKeysProvider>(
        config: GossipConfig,
        channel_id: [u8; 32],
        secret_key: [u8; 32],
        keys_provider: P,
    ) -> Result<Self> {
        // Build the KMS signing key first: `ed25519_from_bytes` zeroizes
        // its input.
        let signing_key = Ed25519Key::from_bytes(&secret_key);
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

        let gossipsub_config = gossipsub::ConfigBuilder::default()
            .validation_mode(gossipsub::ValidationMode::Strict)
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

        let mut swarm = SwarmBuilder::with_existing_identity(keypair)
            .with_tokio()
            .with_quic()
            .with_behaviour(|_key| GossipBehaviour {
                gossipsub: gossipsub_behaviour,
                identify: identify_behaviour,
            })
            .expect("behaviour constructor is infallible")
            .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(Duration::from_secs(60)))
            .build();

        swarm
            .listen_on(listen_addr)
            .context("Failed to listen on gossip address")?;

        // Fail fast on bind errors: wait for the first listen address.
        let listen_addrs = wait_for_listen_addr(&mut swarm).await?;
        info!("Gossip listening on {listen_addrs:?} as {local_peer_id}");

        let topic = gossipsub::IdentTopic::new(announcements_topic(&channel_id));
        swarm
            .behaviour_mut()
            .gossipsub
            .subscribe(&topic)
            .map_err(|err| anyhow!("Failed to subscribe to announcements topic: {err}"))?;

        for addr in &bootstrap {
            if let Err(err) = swarm.dial(addr.clone()) {
                warn!("Failed to dial gossip bootstrap peer {addr}: {err}");
            }
        }

        let (connected_tx, connected_rx) = watch::channel(Vec::new());
        let driver_cancellation = CancellationToken::new();

        // Accredited keys arrive over a watch fed by a separate refresher
        // task, so the swarm loop never awaits an HTTP fetch.
        let (keys_tx, keys_rx) = watch::channel(HashSet::new());
        let (refresh_tx, refresh_rx) = mpsc::channel(1);
        tokio::spawn(run_keys_refresher(
            keys_provider,
            config.keys_refresh_interval,
            refresh_rx,
            keys_tx,
        ));

        let driver = tokio::spawn(run_drive_task(DriveTask {
            swarm,
            topic,
            channel_id,
            signing_key,
            own_pubkey: Ed25519Key::from_bytes(&secret_key).public_key().to_bytes(),
            announce_interval: config.announce_interval,
            bootstrap,
            directory: PeerDirectory::default(),
            connected: HashSet::new(),
            dial_backoff: HashMap::new(),
            connected_tx,
            keys_rx,
            refresh_tx,
            cancellation: driver_cancellation.clone(),
        }));
        spawn_death_reminder(driver_cancellation.clone());

        Ok(Self {
            connected_rx,
            driver_cancellation,
            driver,
            listen_addrs,
            local_peer_id,
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
    topic: gossipsub::IdentTopic,
    channel_id: [u8; 32],
    signing_key: Ed25519Key,
    own_pubkey: [u8; 32],
    announce_interval: Duration,
    bootstrap: Vec<Multiaddr>,
    directory: PeerDirectory,
    connected: HashSet<PeerId>,
    /// Per-peer dial backoff: (attempts, earliest next attempt).
    dial_backoff: HashMap<PeerId, (u32, tokio::time::Instant)>,
    connected_tx: watch::Sender<Vec<[u8; 32]>>,
    keys_rx: watch::Receiver<HashSet<[u8; 32]>>,
    refresh_tx: mpsc::Sender<()>,
    cancellation: CancellationToken,
}

impl DriveTask {
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "SwarmEvent is non_exhaustive; only connection and message events are handled"
    )]
    fn on_swarm_event(&mut self, event: SwarmEvent<GossipBehaviourEvent>) {
        match event {
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                self.connected.insert(peer_id);
                self.dial_backoff.remove(&peer_id);
                self.update_connected_watch();
            }
            SwarmEvent::ConnectionClosed {
                peer_id,
                num_established: 0,
                ..
            } => {
                self.connected.remove(&peer_id);
                self.update_connected_watch();
            }
            SwarmEvent::Behaviour(GossipBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                propagation_source,
                message_id,
                message,
            })) => self.on_gossip_message(propagation_source, &message_id, &message),
            _ => {}
        }
    }

    fn on_gossip_message(
        &mut self,
        source: PeerId,
        message_id: &gossipsub::MessageId,
        message: &gossipsub::Message,
    ) {
        use crate::gossip::validation::{Evaluation, evaluate_announcement};

        let evaluation = evaluate_announcement(
            &message.data,
            &self.channel_id,
            &self.own_pubkey,
            &self.keys_rx.borrow(),
            &self.directory,
        );

        let acceptance = match evaluation {
            Evaluation::Reject(reason) => {
                debug!("Rejecting gossip announcement from {source}: {reason:?}");
                gossipsub::MessageAcceptance::Reject
            }
            Evaluation::IgnoreUnknownKey => {
                // Our cached accredited set may be stale; nudge the
                // refresher (it rate-limits internally). Never blocks; a full
                // channel already has a refresh pending, so a dropped nudge is fine.
                _ = self.refresh_tx.try_send(());
                gossipsub::MessageAcceptance::Ignore
            }
            Evaluation::IgnoreOwn | Evaluation::IgnoreStale => gossipsub::MessageAcceptance::Ignore,
            Evaluation::Accept {
                public_key,
                peer_id,
                listen_addrs,
                seq,
            } => {
                info!(
                    "Learned gossip addresses for sequencer {} ({} addrs)",
                    hex::encode(public_key),
                    listen_addrs.len()
                );
                self.directory
                    .upsert(public_key, peer_id, listen_addrs, seq);
                // A fresh entry may map an already-open connection to its
                // key, or name a peer we should dial now.
                self.update_connected_watch();
                self.dial_missing_peers();
                gossipsub::MessageAcceptance::Accept
            }
        };

        let _ = self
            .swarm
            .behaviour_mut()
            .gossipsub
            .report_message_validation_result(message_id, &source, acceptance);
    }

    fn publish_announcement(&mut self) {
        // Announce concrete listener addresses; unspecified IPs (0.0.0.0)
        // are useless to remote peers.
        let addrs: Vec<String> = self
            .swarm
            .listeners()
            .chain(self.swarm.external_addresses())
            .filter(|addr| !is_unspecified(addr))
            .map(ToString::to_string)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .take(MAX_LISTEN_ADDRS)
            .collect();
        if addrs.is_empty() {
            return;
        }

        let announcement = Announcement {
            channel_id: self.channel_id,
            public_key: self.own_pubkey,
            listen_addrs: addrs,
            seq: unix_millis(),
        };
        let bytes = announcement.sign(&self.signing_key).to_bytes();
        if let Err(err) = self
            .swarm
            .behaviour_mut()
            .gossipsub
            .publish(self.topic.clone(), bytes)
        {
            // `InsufficientPeers` while alone is the normal quiet state.
            debug!("Skipping gossip announcement publish: {err}");
        }
    }

    /// Dial every accredited directory entry we are not connected to, with
    /// per-peer exponential backoff. Bootstrap addresses are retried only
    /// while fully disconnected (they may lack a peer id to track).
    fn dial_missing_peers(&mut self) {
        let now = tokio::time::Instant::now();
        let accredited = self.keys_rx.borrow().clone();

        let candidates: Vec<(PeerId, Vec<Multiaddr>)> = self
            .directory
            .iter()
            .filter(|(key, entry)| {
                accredited.contains(*key) && !self.connected.contains(&entry.peer_id)
            })
            .map(|(_, entry)| (entry.peer_id, entry.listen_addrs.clone()))
            .collect();

        for (peer_id, addrs) in candidates {
            if let Some((_, next_attempt)) = self.dial_backoff.get(&peer_id)
                && *next_attempt > now
            {
                continue;
            }
            let (attempts, _) = self.dial_backoff.remove(&peer_id).unwrap_or((0, now));
            let delay = DIAL_BACKOFF_BASE
                .saturating_mul(2_u32.saturating_pow(attempts))
                .min(DIAL_BACKOFF_MAX);
            let next_attempt = now.checked_add(delay).unwrap_or(now);
            self.dial_backoff
                .insert(peer_id, (attempts.saturating_add(1), next_attempt));

            let opts = libp2p::swarm::dial_opts::DialOpts::peer_id(peer_id)
                .addresses(addrs)
                .build();
            if let Err(err) = self.swarm.dial(opts) {
                debug!("Gossip dial of {peer_id} failed to start: {err}");
            }
        }

        if self.connected.is_empty() {
            for addr in self.bootstrap.clone() {
                if let Err(err) = self.swarm.dial(addr.clone()) {
                    debug!("Gossip bootstrap redial of {addr} failed: {err}");
                }
            }
        }
    }

    fn warn_if_isolated(&self, started_at: tokio::time::Instant) {
        // Snapshot the watch once: `connected_accredited` must never
        // re-borrow `keys_rx` while a guard is held.
        let accredited = self.keys_rx.borrow().clone();
        if started_at.elapsed() > NO_PEERS_GRACE
            && accredited.len() > 1
            && self.connected_accredited(&accredited).next().is_none()
        {
            warn!(
                "Channel has {} accredited sequencers but no gossip peers are connected — \
                 check `gossip.bootstrap_peers` in the sequencer config",
                accredited.len()
            );
        }
    }

    /// Connected peers that map to an accredited key via the directory.
    fn connected_accredited<'keys>(
        &'keys self,
        accredited: &'keys HashSet<[u8; 32]>,
    ) -> impl Iterator<Item = [u8; 32]> + 'keys {
        self.connected
            .iter()
            .filter_map(|peer_id| self.directory.pubkey_of(peer_id))
            .filter(move |pubkey| accredited.contains(pubkey))
    }

    fn update_connected_watch(&self) {
        let accredited = self.keys_rx.borrow().clone();
        let mut peers: Vec<[u8; 32]> = self.connected_accredited(&accredited).collect();
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
}

/// Derives the libp2p `PeerId` an Ed25519 public key produces. `None` only
/// for byte strings that are not a valid curve point.
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

    let started_at = tokio::time::Instant::now();
    let mut announce_interval = tokio::time::interval(task.announce_interval);
    let mut dial_interval = tokio::time::interval(DIAL_RETRY_INTERVAL);
    let mut warn_interval = tokio::time::interval(NO_PEERS_WARN_INTERVAL);

    loop {
        tokio::select! {
            event = task.swarm.select_next_some() => task.on_swarm_event(event),
            _ = announce_interval.tick() => task.publish_announcement(),
            _ = dial_interval.tick() => task.dial_missing_peers(),
            _ = warn_interval.tick() => task.warn_if_isolated(started_at),
        }
    }
}

#[expect(
    clippy::integer_division_remainder_used,
    reason = "Generated by select! macro, can't be easily rewritten to avoid this lint"
)]
async fn run_keys_refresher<P: AccreditedKeysProvider>(
    provider: P,
    refresh_interval: Duration,
    mut refresh_rx: mpsc::Receiver<()>,
    keys_tx: watch::Sender<HashSet<[u8; 32]>>,
) {
    /// Minimum spacing for demand-driven (unknown-key) refreshes.
    const MIN_SPACING: Duration = Duration::from_secs(10);
    let mut interval = tokio::time::interval(refresh_interval);
    let mut last_fetch = tokio::time::Instant::now()
        .checked_sub(MIN_SPACING)
        .unwrap_or_else(tokio::time::Instant::now);

    loop {
        tokio::select! {
            _ = interval.tick() => {}
            request = refresh_rx.recv() => {
                if request.is_none() {
                    return; // Drive task gone.
                }
                if last_fetch.elapsed() < MIN_SPACING {
                    continue;
                }
            }
        }
        last_fetch = tokio::time::Instant::now();
        match provider.accredited_keys().await {
            // Errors keep the last known set — never shrink on a fetch hiccup.
            Err(err) => warn!("Failed to refresh accredited keys for gossip: {err:#}"),
            Ok(keys) => {
                keys_tx.send_if_modified(|current| {
                    if *current == keys {
                        false
                    } else {
                        info!("Gossip accredited key set updated ({} keys)", keys.len());
                        *current = keys;
                        true
                    }
                });
            }
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

#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "Protocol is non_exhaustive with many variants; only IP variants matter here"
)]
fn is_unspecified(addr: &Multiaddr) -> bool {
    addr.iter().any(|proto| match proto {
        libp2p::multiaddr::Protocol::Ip4(ip) => ip.is_unspecified(),
        libp2p::multiaddr::Protocol::Ip6(ip) => ip.is_unspecified(),
        _ => false,
    })
}

fn unix_millis() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_millis(),
    )
    .expect("timestamp fits u64")
}

#[cfg(test)]
mod tests {
    use logos_blockchain_key_management_system_service::keys::Ed25519Key;

    use super::*;
    use crate::{config::GossipConfig, gossip::keys_provider::StaticKeysProvider};

    fn test_config() -> GossipConfig {
        GossipConfig {
            listen_addr: "/ip4/127.0.0.1/udp/0/quic-v1".to_owned(),
            bootstrap_peers: vec![],
            announce_interval: std::time::Duration::from_secs(1),
            keys_refresh_interval: std::time::Duration::from_secs(3600),
        }
    }

    #[test]
    fn libp2p_identity_matches_kms_public_key() {
        // The PeerId derived from an announcement's public_key must equal
        // the PeerId the same secret produces as a libp2p identity.
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
            StaticKeysProvider(std::collections::HashSet::new()),
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
                StaticKeysProvider(std::collections::HashSet::new()),
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
                StaticKeysProvider(std::collections::HashSet::new()),
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
            StaticKeysProvider(std::collections::HashSet::new()),
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
