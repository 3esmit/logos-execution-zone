//! Pure decision function for inbound announcements: everything except the
//! directory upsert and the gossipsub report, so the whole pipeline is
//! testable without a swarm.

use std::collections::HashSet;

use libp2p::{Multiaddr, PeerId};

use crate::gossip::{
    announcement::{RejectReason, SignedAnnouncement},
    directory::PeerDirectory,
    network::peer_id_from_ed25519,
};

#[derive(Debug)]
pub enum Evaluation {
    /// Structurally invalid or forged: penalize the propagating peer.
    Reject(RejectReason),
    /// Signed by a key outside our (possibly stale) accredited set.
    IgnoreUnknownKey,
    /// Our own announcement echoed back (or replayed by another peer).
    IgnoreOwn,
    /// At or below the directory's stored seq for this key.
    IgnoreStale,
    /// Fresh and accredited: caller upserts the directory and dials.
    Accept {
        public_key: [u8; 32],
        peer_id: PeerId,
        listen_addrs: Vec<Multiaddr>,
        seq: u64,
    },
}

#[must_use]
pub fn evaluate_announcement(
    data: &[u8],
    channel_id: &[u8; 32],
    own_pubkey: &[u8; 32],
    accredited: &HashSet<[u8; 32]>,
    directory: &PeerDirectory,
) -> Evaluation {
    let announcement = match SignedAnnouncement::decode_and_verify(data, channel_id) {
        Ok(announcement) => announcement,
        Err(reason) => return Evaluation::Reject(reason),
    };

    if &announcement.public_key == own_pubkey {
        return Evaluation::IgnoreOwn;
    }
    if !accredited.contains(&announcement.public_key) {
        return Evaluation::IgnoreUnknownKey;
    }

    let listen_addrs: Vec<Multiaddr> = match announcement
        .listen_addrs
        .iter()
        .map(|addr| addr.parse())
        .collect()
    {
        Ok(addrs) => addrs,
        Err(_err) => return Evaluation::Reject(RejectReason::Undecodable),
    };

    // The key was verified as a valid curve point by `decode_and_verify`.
    let Some(peer_id) = peer_id_from_ed25519(&announcement.public_key) else {
        return Evaluation::Reject(RejectReason::BadSignature);
    };

    if directory
        .iter()
        .any(|(key, entry)| key == &announcement.public_key && entry.seq >= announcement.seq)
    {
        return Evaluation::IgnoreStale;
    }

    Evaluation::Accept {
        public_key: announcement.public_key,
        peer_id,
        listen_addrs,
        seq: announcement.seq,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use logos_blockchain_key_management_system_service::keys::Ed25519Key;

    use super::*;
    use crate::gossip::announcement::{Announcement, RejectReason};

    const CHANNEL: [u8; 32] = [1; 32];
    const OWN: [u8; 32] = [0xAA; 32];

    fn key() -> Ed25519Key {
        Ed25519Key::from_bytes(&[7; 32])
    }

    fn bytes(key: &Ed25519Key, seq: u64) -> Vec<u8> {
        Announcement {
            channel_id: CHANNEL,
            public_key: key.public_key().to_bytes(),
            listen_addrs: vec!["/ip4/127.0.0.1/udp/7070/quic-v1".to_owned()],
            seq,
        }
        .sign(key)
        .to_bytes()
    }

    fn accredited(key: &Ed25519Key) -> HashSet<[u8; 32]> {
        HashSet::from([key.public_key().to_bytes()])
    }

    #[test]
    fn accredited_fresh_announcement_is_accepted() {
        let key = key();
        let directory = PeerDirectory::default();
        let evaluation = evaluate_announcement(
            &bytes(&key, 1),
            &CHANNEL,
            &OWN,
            &accredited(&key),
            &directory,
        );
        let Evaluation::Accept {
            public_key,
            listen_addrs,
            seq,
            ..
        } = evaluation
        else {
            panic!("expected Accept, got {evaluation:?}");
        };
        assert_eq!(public_key, key.public_key().to_bytes());
        assert_eq!(listen_addrs.len(), 1);
        assert_eq!(seq, 1);
    }

    #[test]
    fn structural_failure_is_reject() {
        let directory = PeerDirectory::default();
        assert!(matches!(
            evaluate_announcement(b"junk", &CHANNEL, &OWN, &HashSet::new(), &directory),
            Evaluation::Reject(RejectReason::Undecodable)
        ));
    }

    #[test]
    fn unknown_key_is_ignored_not_rejected() {
        let key = key();
        let directory = PeerDirectory::default();
        assert!(matches!(
            evaluate_announcement(&bytes(&key, 1), &CHANNEL, &OWN, &HashSet::new(), &directory),
            Evaluation::IgnoreUnknownKey
        ));
    }

    #[test]
    fn own_echoed_announcement_is_ignored() {
        let key = key();
        let own = key.public_key().to_bytes();
        let directory = PeerDirectory::default();
        assert!(matches!(
            evaluate_announcement(
                &bytes(&key, 1),
                &CHANNEL,
                &own,
                &accredited(&key),
                &directory
            ),
            Evaluation::IgnoreOwn
        ));
    }

    #[test]
    fn replayed_seq_is_stale() {
        let key = key();
        let mut directory = PeerDirectory::default();
        let Evaluation::Accept {
            public_key,
            peer_id,
            listen_addrs,
            seq,
        } = evaluate_announcement(
            &bytes(&key, 5),
            &CHANNEL,
            &OWN,
            &accredited(&key),
            &directory,
        )
        else {
            panic!("expected Accept");
        };
        directory.upsert(public_key, peer_id, listen_addrs, seq);
        assert!(matches!(
            evaluate_announcement(
                &bytes(&key, 5),
                &CHANNEL,
                &OWN,
                &accredited(&key),
                &directory
            ),
            Evaluation::IgnoreStale
        ));
    }

    #[test]
    fn unparseable_multiaddr_is_reject() {
        let key = key();
        let announcement_bytes = Announcement {
            channel_id: CHANNEL,
            public_key: key.public_key().to_bytes(),
            listen_addrs: vec!["not a multiaddr".to_owned()],
            seq: 1,
        }
        .sign(&key)
        .to_bytes();
        let directory = PeerDirectory::default();
        assert!(matches!(
            evaluate_announcement(
                &announcement_bytes,
                &CHANNEL,
                &OWN,
                &accredited(&key),
                &directory
            ),
            Evaluation::Reject(RejectReason::Undecodable)
        ));
    }
}
