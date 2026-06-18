use lee_core::{
    account::AccountId,
    program::{PdaSeed, ProgramId},
};
use serde::{Deserialize, Serialize};

/// Raw 32-byte zone (channel) id; the host maps it to the zone-sdk `ChannelId`.
pub type ZoneId = [u8; 32];

const OUTBOX_SEED_DOMAIN: [u8; 32] = *b"/LEZ/v0.3/CrossZoneOutbox/00000/";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Instruction {
    /// Records an outbound cross-zone message as a write to a self-owned PDA.
    ///
    /// Required accounts (1):
    /// - Outbox PDA account
    Emit {
        target_zone: ZoneId,
        target_program_id: ProgramId,
        payload: Vec<u8>,
    },
}

/// PDA holding one emitted message, keyed by destination zone and a per-zone
/// ordinal.
#[must_use]
pub fn outbox_pda(outbox_id: ProgramId, target_zone: &ZoneId, ordinal: u32) -> AccountId {
    AccountId::for_public_pda(&outbox_id, &outbox_seed(target_zone, ordinal))
}

fn outbox_seed(target_zone: &ZoneId, ordinal: u32) -> PdaSeed {
    use risc0_zkvm::sha::{Impl, Sha256 as _};

    let mut bytes = Vec::with_capacity(OUTBOX_SEED_DOMAIN.len() + target_zone.len() + 4);
    bytes.extend_from_slice(&OUTBOX_SEED_DOMAIN);
    bytes.extend_from_slice(target_zone);
    bytes.extend_from_slice(&ordinal.to_le_bytes());

    let seed: [u8; 32] = Impl::hash_bytes(&bytes)
        .as_bytes()
        .try_into()
        .unwrap_or_else(|_| unreachable!());
    PdaSeed::new(seed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outbox_pda_is_unique_per_zone_and_ordinal() {
        let id: ProgramId = [3; 8];
        let zone_a = [1; 32];
        let zone_b = [2; 32];

        assert_eq!(outbox_pda(id, &zone_a, 0), outbox_pda(id, &zone_a, 0));
        assert_ne!(outbox_pda(id, &zone_a, 0), outbox_pda(id, &zone_a, 1));
        assert_ne!(outbox_pda(id, &zone_a, 0), outbox_pda(id, &zone_b, 0));
    }
}
