//! "Genesis follows config" checks for both the from-scratch and prebuilt `TestContext` paths.
//! Need Docker/Bedrock, like the integration tests.
#![expect(clippy::tests_outside_test_module, reason = "It's a tests module")]

use anyhow::{Context as _, Result};
use lee::{AccountId, PublicKey};
use sequencer_service_rpc::RpcClient as _;
use test_fixtures::{
    MultiZoneTestContextBuilder, TestContext, ZoneTestContextBuilder,
    config::{
        MultiNodeTestContextConfig, bedrock_channel_id_b, default_private_accounts_for_wallet,
        default_public_accounts_for_wallet,
    },
    verify_commitment_is_in_state,
};

/// Default fixture dependencies must select the development genesis program set.
/// This check needs no Docker and catches transitive Testnet feature activation.
#[test]
fn default_fixture_registers_genesis_programs() {
    let mut state = testnet_initial_state::initial_state_for_profile(
        testnet_initial_state::InitialStateProfile::default(),
    );
    let registered = state.program_ids();
    for program in [
        programs::wrapped_token(),
        programs::ping_sender(),
        programs::ping_receiver(),
        programs::bridge_lock(),
        programs::cross_zone_inbox(),
        programs::sequencer_stake(),
    ] {
        assert!(
            registered.contains(&program.id()),
            "fixture genesis must register program {:?}",
            program.id(),
        );
    }
    state
        .transition_from_public_transaction(&common::transaction::clock_invocation(0), 1, 0)
        .expect("fixture genesis clock must match the selected state");
}

/// Builds from genesis (no prebuilt database) and checks the on-chain state follows the config.
#[tokio::test]
async fn genesis_from_scratch_follows_config() -> Result<()> {
    let config = MultiNodeTestContextConfig {
        bedrock_channel: bedrock_channel_id_b(),
        ..MultiNodeTestContextConfig::default()
    };
    let ctx = MultiZoneTestContextBuilder::default()
        .with_zone(ZoneTestContextBuilder::new(config).from_scratch())
        .build()
        .await?;

    assert_context_follows_config(&ctx).await
}

/// Loads the prebuilt database via [`TestContext::new`] and checks the restored state follows the
/// config — i.e. the dump round-trips and all expected data persists.
#[tokio::test]
async fn prebuilt_context_follows_config() -> Result<()> {
    let ctx = TestContext::new().await?;
    assert_context_follows_config(&ctx).await
}

/// Assert every configured default account is funded with its configured balance, and every
/// configured private account's commitment is present in sequencer state. (The wallet also has an
/// unfunded default root account, so we check the configured accounts specifically.)
async fn assert_context_follows_config(ctx: &TestContext) -> Result<()> {
    for (private_key, expected_balance) in default_public_accounts_for_wallet() {
        let account_id = AccountId::from(&PublicKey::new_from_private_key(&private_key));
        let balance = ctx
            .sequencer_client()
            .get_account_balance(account_id)
            .await?;
        assert_eq!(
            balance, expected_balance,
            "public account {account_id} balance"
        );
    }

    for account in default_private_accounts_for_wallet() {
        let account_id = account.account_id();
        let commitment = ctx
            .wallet()
            .get_private_account_commitment(account_id)
            .with_context(|| format!("commitment for private account {account_id}"))?;
        assert!(
            verify_commitment_is_in_state(commitment, ctx.sequencer_client()).await,
            "private commitment for {account_id} should be in sequencer state"
        );
    }

    Ok(())
}
