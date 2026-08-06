//! The signed address announcement gossiped on the per-channel topic.
//!
//! Deliberately libp2p-free: pure wire format, signed with the bedrock
//! Ed25519 key so receivers validate against the channel's accredited set.

use borsh::{BorshDeserialize, BorshSerialize};
use logos_blockchain_core::mantle::ops::channel::Ed25519PublicKey;
use logos_blockchain_key_management_system_service::keys::{Ed25519Key, Ed25519Signature};

/// Caps enforced at validation; a violating message is rejected outright.
pub const MAX_LISTEN_ADDRS: usize = 8;
pub const MAX_ADDR_LEN: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Announcement {
    /// Cross-channel replay guard: must equal the topic's channel.
    pub channel_id: [u8; 32],
    /// Announcer's bedrock Ed25519 public key.
    pub public_key: [u8; 32],
    /// Multiaddrs the announcer listens on.
    pub listen_addrs: Vec<String>,
    /// Freshness: unix millis at signing; receivers keep only the highest
    /// per key.
    pub seq: u64,
}

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct SignedAnnouncement {
    pub announcement: Announcement,
    /// Ed25519 signature over `borsh(announcement)`.
    pub signature: [u8; 64],
}

/// Why a message must not be propagated (gossipsub `Reject`). Staleness and
/// unknown keys are `Ignore`, decided by the caller as they are not
/// structural faults of the message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RejectReason {
    Undecodable,
    WrongChannel,
    TooManyAddrs,
    AddrTooLong,
    BadSignature,
}

impl Announcement {
    #[must_use]
    pub fn sign(self, key: &Ed25519Key) -> SignedAnnouncement {
        let payload = borsh::to_vec(&self).expect("announcement serialization cannot fail");
        let signature = key.sign_payload(&payload).to_bytes();
        SignedAnnouncement {
            announcement: self,
            signature,
        }
    }
}

impl SignedAnnouncement {
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        borsh::to_vec(self).expect("announcement serialization cannot fail")
    }

    /// Structural & signature validation.
    /// Accreditation and staleness are the caller's checks.
    pub fn decode_and_verify(
        bytes: &[u8],
        expected_channel: &[u8; 32],
    ) -> Result<Announcement, RejectReason> {
        let signed: Self = borsh::from_slice(bytes).map_err(|_err| RejectReason::Undecodable)?;
        let announcement = &signed.announcement;

        if &announcement.channel_id != expected_channel {
            return Err(RejectReason::WrongChannel);
        }
        if announcement.listen_addrs.len() > MAX_LISTEN_ADDRS {
            return Err(RejectReason::TooManyAddrs);
        }
        if announcement
            .listen_addrs
            .iter()
            .any(|addr| addr.len() > MAX_ADDR_LEN)
        {
            return Err(RejectReason::AddrTooLong);
        }

        let public_key = Ed25519PublicKey::from_bytes(&announcement.public_key)
            .map_err(|_err| RejectReason::BadSignature)?;
        let payload = borsh::to_vec(announcement).expect("announcement serialization cannot fail");
        let signature = Ed25519Signature::from_bytes(&signed.signature);
        public_key
            .verify(&payload, &signature)
            .map_err(|_err| RejectReason::BadSignature)?;

        Ok(signed.announcement)
    }
}

/// `GossipSub` topic carrying [`SignedAnnouncement`]s for a channel.
#[must_use]
pub fn announcements_topic(channel_id: &[u8; 32]) -> String {
    format!("/lez/{}/v1/announcements", hex::encode(channel_id))
}

#[cfg(test)]
mod tests {
    use logos_blockchain_key_management_system_service::keys::Ed25519Key;

    use super::*;

    const CHANNEL: [u8; 32] = [1; 32];

    fn signed(key: &Ed25519Key, addrs: Vec<String>) -> SignedAnnouncement {
        Announcement {
            channel_id: CHANNEL,
            public_key: key.public_key().to_bytes(),
            listen_addrs: addrs,
            seq: 42,
        }
        .sign(key)
    }

    #[test]
    fn round_trip_verifies() {
        let key = Ed25519Key::from_bytes(&[7; 32]);
        let bytes = signed(&key, vec!["/ip4/127.0.0.1/udp/7070/quic-v1".into()]).to_bytes();
        let announcement = SignedAnnouncement::decode_and_verify(&bytes, &CHANNEL).unwrap();
        assert_eq!(announcement.seq, 42);
        assert_eq!(announcement.public_key, key.public_key().to_bytes());
    }

    #[test]
    fn garbage_is_undecodable() {
        assert_eq!(
            SignedAnnouncement::decode_and_verify(b"not borsh", &CHANNEL),
            Err(RejectReason::Undecodable)
        );
    }

    #[test]
    fn wrong_channel_is_rejected() {
        let key = Ed25519Key::from_bytes(&[7; 32]);
        let bytes = signed(&key, vec![]).to_bytes();
        assert_eq!(
            SignedAnnouncement::decode_and_verify(&bytes, &[2; 32]),
            Err(RejectReason::WrongChannel)
        );
    }

    #[test]
    fn tampered_payload_fails_signature() {
        let key = Ed25519Key::from_bytes(&[7; 32]);
        let mut announcement = signed(&key, vec![]);
        announcement.announcement.seq = 43; // signature no longer covers this
        assert_eq!(
            SignedAnnouncement::decode_and_verify(&announcement.to_bytes(), &CHANNEL),
            Err(RejectReason::BadSignature)
        );
    }

    #[test]
    fn signature_from_other_key_fails() {
        let key = Ed25519Key::from_bytes(&[7; 32]);
        let other = Ed25519Key::from_bytes(&[8; 32]);
        // Claims key's identity but signed by other.
        let bytes = Announcement {
            channel_id: CHANNEL,
            public_key: key.public_key().to_bytes(),
            listen_addrs: vec![],
            seq: 1,
        }
        .sign(&other)
        .to_bytes();
        assert_eq!(
            SignedAnnouncement::decode_and_verify(&bytes, &CHANNEL),
            Err(RejectReason::BadSignature)
        );
    }

    #[test]
    fn too_many_addrs_rejected() {
        let key = Ed25519Key::from_bytes(&[7; 32]);
        let addrs = vec!["/ip4/127.0.0.1/udp/1/quic-v1".to_owned(); MAX_LISTEN_ADDRS + 1];
        let bytes = signed(&key, addrs).to_bytes();
        assert_eq!(
            SignedAnnouncement::decode_and_verify(&bytes, &CHANNEL),
            Err(RejectReason::TooManyAddrs)
        );
    }

    #[test]
    fn oversized_addr_rejected() {
        let key = Ed25519Key::from_bytes(&[7; 32]);
        let bytes = signed(&key, vec!["a".repeat(MAX_ADDR_LEN + 1)]).to_bytes();
        assert_eq!(
            SignedAnnouncement::decode_and_verify(&bytes, &CHANNEL),
            Err(RejectReason::AddrTooLong)
        );
    }

    #[test]
    fn topic_is_channel_scoped() {
        assert_eq!(
            announcements_topic(&CHANNEL),
            format!("/lez/{}/v1/announcements", hex::encode(CHANNEL))
        );
    }
}
