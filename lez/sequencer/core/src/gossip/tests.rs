//! Multi-node integration tests over real QUIC on 127.0.0.1.

use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

use logos_blockchain_key_management_system_service::keys::Ed25519Key;

use crate::{
    config::GossipConfig,
    gossip::{Libp2pNetwork, PeerNetworkTrait as _, keys_provider::StaticKeysProvider},
};

const CHANNEL: [u8; 32] = [1; 32];

fn pubkey(secret: [u8; 32]) -> [u8; 32] {
    Ed25519Key::from_bytes(&secret).public_key().to_bytes()
}

async fn start_node(
    secret: [u8; 32],
    accredited: HashSet<[u8; 32]>,
    bootstrap: Vec<String>,
) -> Libp2pNetwork {
    let config = GossipConfig {
        listen_addr: "/ip4/127.0.0.1/udp/0/quic-v1".to_owned(),
        bootstrap_peers: bootstrap,
        announce_interval: Duration::from_millis(500),
        keys_refresh_interval: Duration::from_secs(3600),
    };
    Libp2pNetwork::start(config, CHANNEL, secret, StaticKeysProvider(accredited))
        .await
        .expect("node should start")
}

/// Polls `condition` until it holds or `timeout` elapses.
async fn wait_for(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    false
}

#[tokio::test]
async fn transitive_discovery_connects_all_accredited_nodes() {
    let secrets = [[10; 32], [11; 32], [12; 32]];
    let accredited: HashSet<[u8; 32]> = secrets.iter().map(|secret| pubkey(*secret)).collect();

    // A is the only bootstrap point; B and C never learn each other's
    // addresses from config.
    let node_a = start_node(secrets[0], accredited.clone(), vec![]).await;
    let a_addr = node_a.listen_addrs()[0].to_string();
    let node_b = start_node(secrets[1], accredited.clone(), vec![a_addr.clone()]).await;
    let node_c = start_node(secrets[2], accredited.clone(), vec![a_addr]).await;

    // C must discover B via A's gossip and dial it directly.
    assert!(
        wait_for(Duration::from_secs(30), || {
            node_c.connected_peers().contains(&pubkey(secrets[1]))
        })
        .await,
        "C never connected to B via gossip; C sees {:?}",
        node_c.connected_peers()
    );
    assert!(node_b.connected_peers().contains(&pubkey(secrets[2])));
    drop((node_a, node_b, node_c));
}

#[tokio::test]
async fn non_accredited_node_is_never_a_connected_peer() {
    let accredited_secrets = [[20; 32], [21; 32]];
    let outsider_secret = [22; 32];
    let accredited: HashSet<[u8; 32]> = accredited_secrets
        .iter()
        .map(|secret| pubkey(*secret))
        .collect();

    let node_a = start_node(accredited_secrets[0], accredited.clone(), vec![]).await;
    let a_addr = node_a.listen_addrs()[0].to_string();
    let node_b = start_node(
        accredited_secrets[1],
        accredited.clone(),
        vec![a_addr.clone()],
    )
    .await;
    // The outsider considers everyone accredited and announces eagerly —
    // but its own key is in nobody's set.
    let outsider = start_node(outsider_secret, accredited.clone(), vec![a_addr]).await;

    assert!(
        wait_for(Duration::from_secs(30), || {
            node_a
                .connected_peers()
                .contains(&pubkey(accredited_secrets[1]))
        })
        .await,
        "accredited pair should connect"
    );
    // Give the outsider ample time to announce, then confirm it was ignored.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(
        !node_a.connected_peers().contains(&pubkey(outsider_secret)),
        "outsider must not appear as a connected accredited peer"
    );
    assert!(!node_b.connected_peers().contains(&pubkey(outsider_secret)));
    drop((node_a, node_b, outsider));
}
