use lee_core::{
    account::AccountId,
    program::{PdaSeed, ProgramId},
};
use serde::{Deserialize, Serialize};

const PING_RECORD_SEED: [u8; 32] = *b"/LEZ/v0.3/PingRecord/0000000000/";
const SENDER_CONFIG_SEED: [u8; 32] = *b"/LEZ/v0.3/PingSenderCfg/0000000/";

/// Instruction delivered to `ping_receiver` by the inbox: record the payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReceiverInstruction {
    Record { payload: Vec<u8> },
}

/// Instruction to `ping_sender`. `Send`'s emission fields are forwarded verbatim
/// into `cross_zone_outbox::Instruction::Emit`.
///
/// Variants are append-only. risc0 serde encodes the variant as a bare leading
/// tag word, so inserting one ahead of `Send` shifts every existing encoding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SenderInstruction {
    /// Emit a cross-zone message through the pinned outbox.
    ///
    /// Required accounts (2): the sender config PDA, then the outbox PDA.
    Send {
        target_zone: [u8; 32],
        target_program_id: ProgramId,
        target_accounts: Vec<[u8; 32]>,
        payload: Vec<u8>,
        ordinal: u32,
    },
    /// Pins the outbox program, written once into a default config PDA at
    /// genesis. A re-run naming a different outbox is refused; an identical one
    /// is a no-op, which is what genesis replay does.
    ///
    /// Required accounts (1): the sender config PDA.
    InitConfig { outbox_program_id: ProgramId },
}

/// The account a `ping_receiver` records the latest delivered payload into.
#[must_use]
pub fn ping_record_pda(receiver_id: ProgramId) -> AccountId {
    AccountId::for_public_pda(&receiver_id, &ping_record_seed())
}

/// Seed of the record PDA, exposed so the guest can claim the account.
#[must_use]
pub const fn ping_record_seed() -> PdaSeed {
    PdaSeed::new(PING_RECORD_SEED)
}

/// PDA holding the outbox program id, seeded at genesis so the guest can pin the
/// program it chains into without importing the outbox image id.
#[must_use]
pub fn sender_config_account_id(sender_id: ProgramId) -> AccountId {
    AccountId::for_public_pda(&sender_id, &sender_config_seed())
}

#[must_use]
pub const fn sender_config_seed() -> PdaSeed {
    PdaSeed::new(SENDER_CONFIG_SEED)
}

/// Encodes the pinned outbox program id for the config account's data.
#[must_use]
pub fn outbox_bytes(outbox_program_id: ProgramId) -> [u8; 32] {
    let mut bytes = [0_u8; 32];
    for (word, chunk) in outbox_program_id.iter().zip(bytes.chunks_exact_mut(4)) {
        chunk.copy_from_slice(&word.to_le_bytes());
    }
    bytes
}

/// Decodes the pinned outbox program id from the config account's data.
#[must_use]
pub fn read_outbox(data: &[u8]) -> Option<ProgramId> {
    if data.len() < 32 {
        return None;
    }
    let mut outbox_program_id = [0_u32; 8];
    for (word, chunk) in outbox_program_id.iter_mut().zip(data[..32].chunks_exact(4)) {
        *word = u32::from_le_bytes(chunk.try_into().unwrap_or_else(|_| unreachable!()));
    }
    Some(outbox_program_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `extract_emission` decodes `Send` off peer transactions, so its tag word is
    /// wire format: a variant inserted ahead of it would silently shift every
    /// existing encoding.
    #[test]
    fn send_is_the_first_variant() {
        let send = SenderInstruction::Send {
            target_zone: [7; 32],
            target_program_id: [1; 8],
            target_accounts: vec![],
            payload: vec![],
            ordinal: 0,
        };
        let words = risc0_zkvm::serde::to_vec(&send).expect("Send serializes");
        assert_eq!(words[0], 0);
    }

    #[test]
    fn outbox_id_round_trips() {
        let outbox: ProgramId = [9; 8];
        assert_eq!(read_outbox(&outbox_bytes(outbox)), Some(outbox));
    }
}
