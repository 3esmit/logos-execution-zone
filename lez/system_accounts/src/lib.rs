//! This crate provides system accounts used by LEZ.

use std::{collections::BTreeMap, str::FromStr as _};

use clock_core::ClockAccountData;
use lee_core::account::{Account, AccountId, Nonce};

/// Minimum summed stake for a Bedrock sequencer key to be a committee candidate.
pub const DEFAULT_MINIMUM_SEQUENCER_STAKE: u128 = 1_000_000;

/// Channel administration defaults.
///
/// Slots, not seconds (1 slot = 1s on the current devnet): 20-slot turns,
/// reclaimed after 10 idle slots if a sequencer stops posting — non-zero so
/// round robin can move on when a committee has more than one accredited key.
/// A lone-signature threshold still suffices for config changes.
pub const DEFAULT_SEQUENCER_POSTING_TIMEFRAME: u32 = 20;
pub const DEFAULT_SEQUENCER_POSTING_TIMEOUT: u32 = 10;
pub const DEFAULT_SEQUENCER_CONFIGURATION_THRESHOLD: u16 = 1;
pub const DEFAULT_SEQUENCER_WITHDRAW_THRESHOLD: u16 = 1;

#[must_use]
pub fn pinata_account_id() -> AccountId {
    // TODO: Use derivation from a public key?
    AccountId::from_str("EfQhKQAkX2FJiwNii2WFQsGndjvF1Mzd7RuVe7QdPLw7")
        .expect("Pinata program id should be valid")
}

#[must_use]
pub fn pinata_account() -> Account {
    Account {
        program_owner: programs::pinata().id(),
        balance: 1_500_000,
        // Difficulty: 3
        data: vec![3; 33].try_into().expect("Should fit"),
        nonce: Nonce::default(),
    }
}

#[must_use]
pub fn faucet_account_id() -> AccountId {
    faucet_core::compute_faucet_account_id(programs::faucet().id())
}

#[must_use]
pub fn faucet_account() -> Account {
    Account {
        program_owner: programs::authenticated_transfer().id(),
        balance: u128::MAX,
        ..Account::default()
    }
}

#[must_use]
pub fn bridge_account_id() -> AccountId {
    bridge_core::compute_bridge_account_id(programs::bridge().id())
}

#[must_use]
pub fn bridge_account() -> Account {
    Account {
        program_owner: programs::authenticated_transfer().id(),
        ..Account::default()
    }
}

#[must_use]
pub const fn clock_account_ids() -> [AccountId; 3] {
    clock_core::CLOCK_PROGRAM_ACCOUNT_IDS
}

#[must_use]
pub fn sequencer_stake_config_account_id() -> AccountId {
    sequencer_stake_core::sequencer_stake_config_account_id(programs::sequencer_stake().id())
}

/// `entries` is empty in the shared base state (no sequencer key is known there).
///
/// A running sequencer's `build_initial_state` replaces this account with one
/// whose `entries` map also carries the bootstrap sequencer's own entry,
/// alongside seeding [`sequencer_stake_bootstrap_account`].
#[must_use]
pub fn sequencer_stake_config_account(
    entries: BTreeMap<sequencer_stake_core::SequencerKey, sequencer_stake_core::SequencerEntry>,
) -> Account {
    Account {
        program_owner: programs::sequencer_stake().id(),
        data: sequencer_stake_core::SequencerStakeConfig {
            minimum_sequencer_stake: DEFAULT_MINIMUM_SEQUENCER_STAKE,
            entries,
        }
        .to_bytes()
        .try_into()
        .expect("sequencer stake config data should fit"),
        ..Account::default()
    }
}

/// Genesis stake for the sequencer that bootstraps the channel.
///
/// Seeded at the LEZ account named in the sequencer's config, so the operator
/// can sign for it.
#[must_use]
pub fn sequencer_stake_bootstrap_account(
    sequencer_key: sequencer_stake_core::SequencerKey,
) -> Account {
    Account {
        program_owner: programs::sequencer_stake().id(),
        balance: DEFAULT_MINIMUM_SEQUENCER_STAKE,
        data: sequencer_stake_core::StakeRecord {
            sequencer_key,
            pending_unstake: None,
        }
        .to_bytes()
        .try_into()
        .expect("stake record should fit"),
        ..Account::default()
    }
}

/// The config-account entry backing [`sequencer_stake_bootstrap_account`].
#[must_use]
pub const fn sequencer_stake_bootstrap_entry(
    account_id: AccountId,
) -> sequencer_stake_core::SequencerEntry {
    sequencer_stake_core::SequencerEntry {
        account_id,
        total_staked: DEFAULT_MINIMUM_SEQUENCER_STAKE,
        total_pending_unstake: 0,
    }
}

#[must_use]
pub fn clock_account() -> Account {
    Account {
        program_owner: programs::clock().id(),
        data: ClockAccountData {
            block_id: 0,
            timestamp: 0,
        }
        .to_bytes()
        .try_into()
        .expect("Clock account data should fit"),
        ..Account::default()
    }
}
