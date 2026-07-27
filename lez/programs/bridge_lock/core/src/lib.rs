//! Core types for the bridge-lock program, the source side of the cross-zone
//! token bridge. A holder locks part of their balance into an escrow and emits a
//! cross-zone message minting the wrapped token on the target zone.

use lee_core::{
    account::AccountId,
    program::{PdaSeed, ProgramId},
};
use serde::{Deserialize, Serialize};

const ESCROW_SEED_DOMAIN: [u8; 32] = *b"/LEZ/v0.3/BridgeLockEscrow/0000/";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Instruction {
    /// Lock `amount` of the holder's balance and emit a cross-zone message
    /// minting the wrapped token on `target_zone`. The emission fields mirror
    /// `cross_zone_outbox::Instruction::Emit` so the watcher reads them directly.
    ///
    /// Required accounts (3): holder holding (authorized), escrow PDA, outbox PDA.
    Lock {
        amount: u128,
        target_zone: [u8; 32],
        target_program_id: ProgramId,
        target_accounts: Vec<[u8; 32]>,
        payload: Vec<u8>,
        outbox_program_id: ProgramId,
        ordinal: u32,
    },
}

/// PDA accumulating all locked balance on this zone.
#[must_use]
pub fn escrow_account_id(bridge_lock_id: ProgramId) -> AccountId {
    AccountId::for_public_pda(&bridge_lock_id, &escrow_seed())
}

#[must_use]
pub const fn escrow_seed() -> PdaSeed {
    PdaSeed::new(ESCROW_SEED_DOMAIN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escrow_is_stable() {
        let id: ProgramId = [4; 8];
        assert_eq!(escrow_account_id(id), escrow_account_id(id));
    }
}
