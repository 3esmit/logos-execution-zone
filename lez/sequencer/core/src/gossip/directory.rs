//! Latest validated announcement per accredited key. Entries leave only via
//! [`PeerDirectory::retain_keys`] (accredited-set changes), never by
//! timeout — a silent peer's last known addresses stay dialable.

use std::collections::{HashMap, HashSet};

use libp2p::{Multiaddr, PeerId};

pub struct PeerEntry {
    pub peer_id: PeerId,
    pub listen_addrs: Vec<Multiaddr>,
    pub seq: u64,
}

/// What [`PeerDirectory::upsert`] did with an announcement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpsertOutcome {
    /// Newer than anything stored for this key; entry replaced.
    Fresh,
    /// At or below the stored seq; entry untouched.
    Stale,
}

#[derive(Default)]
pub struct PeerDirectory {
    entries: HashMap<[u8; 32], PeerEntry>,
}

impl PeerDirectory {
    pub fn upsert(
        &mut self,
        public_key: [u8; 32],
        peer_id: PeerId,
        listen_addrs: Vec<Multiaddr>,
        seq: u64,
    ) -> UpsertOutcome {
        match self.entries.get(&public_key) {
            Some(entry) if entry.seq >= seq => UpsertOutcome::Stale,
            _ => {
                self.entries.insert(
                    public_key,
                    PeerEntry {
                        peer_id,
                        listen_addrs,
                        seq,
                    },
                );
                UpsertOutcome::Fresh
            }
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&[u8; 32], &PeerEntry)> {
        self.entries.iter()
    }

    /// The stored freshness seq for `public_key`, if an entry exists.
    #[must_use]
    pub fn seq_of(&self, public_key: &[u8; 32]) -> Option<u64> {
        self.entries.get(public_key).map(|entry| entry.seq)
    }

    #[must_use]
    pub fn pubkey_of(&self, peer_id: &PeerId) -> Option<[u8; 32]> {
        self.entries
            .iter()
            .find(|(_, entry)| &entry.peer_id == peer_id)
            .map(|(key, _)| *key)
    }

    pub fn retain_keys(&mut self, accredited: &HashSet<[u8; 32]>) {
        self.entries.retain(|key, _| accredited.contains(key));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(_n: u8) -> PeerId {
        libp2p::identity::Keypair::generate_ed25519()
            .public()
            .to_peer_id()
    }

    #[test]
    fn upsert_newer_seq_wins_older_is_stale() {
        let mut directory = PeerDirectory::default();
        let id = peer(1);
        assert_eq!(
            directory.upsert([1; 32], id, vec![], 10),
            UpsertOutcome::Fresh
        );
        assert_eq!(
            directory.upsert([1; 32], id, vec![], 10),
            UpsertOutcome::Stale,
            "equal seq is a replay"
        );
        assert_eq!(
            directory.upsert([1; 32], id, vec![], 9),
            UpsertOutcome::Stale
        );
        assert_eq!(
            directory.upsert([1; 32], id, vec![], 11),
            UpsertOutcome::Fresh
        );
        assert_eq!(directory.iter().next().unwrap().1.seq, 11);
    }

    #[test]
    fn pubkey_reverse_lookup() {
        let mut directory = PeerDirectory::default();
        let id = peer(1);
        directory.upsert([3; 32], id, vec![], 1);
        assert_eq!(directory.pubkey_of(&id), Some([3; 32]));
        assert_eq!(directory.pubkey_of(&peer(2)), None);
    }

    #[test]
    fn retain_keys_drops_deaccredited() {
        let mut directory = PeerDirectory::default();
        directory.upsert([1; 32], peer(1), vec![], 1);
        directory.upsert([2; 32], peer(2), vec![], 1);
        directory.retain_keys(&std::collections::HashSet::from([[1; 32]]));
        assert_eq!(directory.iter().count(), 1);
        assert!(directory.iter().all(|(key, _)| key == &[1; 32]));
    }
}
