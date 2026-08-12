#![expect(
    clippy::tests_outside_test_module,
    reason = "top-level test functions are conventional for integration tests"
)]

//! Two sequencers share one channel: both are staked at genesis, so A creates
//! the channel already accrediting `[A, B]`, B joins and syncs, both produce on
//! their turns, and A, B and an indexer converge on the same chain.

use std::time::Duration;

use anyhow::{Context as _, Result, ensure};
use indexer_service_rpc::RpcClient as _;
use integration_tests::{
    config::{self, SequencerPartialConfig},
    init_logger,
    setup::{SequencerSetup, indexer_client, sequencer_client, setup_bedrock_node, setup_indexer},
};
use logos_blockchain_key_management_system_service::keys::{ED25519_SECRET_KEY_SIZE, Ed25519Key};
use sequencer_core::{
    block_publisher::{Ed25519PublicKey, read_channel_state},
    config::{BedrockConfig, GenesisAction},
    sign_genesis_stake,
};
use sequencer_service_rpc::{RpcClient as _, SequencerClient};
use testnet_initial_state::{initial_pub_accounts_private_keys, initial_public_user_accounts};
use tokio::test;

const PHASE_TIMEOUT: Duration = Duration::from_secs(360);
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const TRANSFER_AMOUNT: u128 = 10;
/// ≈4 turn windows past B's join, at `DEFAULT_SEQUENCER_POSTING_TIMEFRAME`
/// (20 slots ≈ 20 s) and 5 s blocks.
const ROTATION_BLOCKS: u64 = 8;

#[test]
async fn multi_sequencer_committee_converges() -> Result<()> {
    init_logger();

    let (_bedrock, bedrock_addr) = setup_bedrock_node()
        .await
        .context("Failed to set up Bedrock node")?;

    // Fixed seeds so both keys can be staked in genesis before either starts.
    let key_a = [0xA1_u8; ED25519_SECRET_KEY_SIZE];
    let key_b = [0xB2_u8; ED25519_SECRET_KEY_SIZE];
    let pub_a = Ed25519Key::from_bytes(&key_a).public_key();
    let pub_b = Ed25519Key::from_bytes(&key_b).public_key();

    // Each operator signs its own stake offchain.
    let genesis = vec![
        founding_stake(0, pub_a.to_bytes(), [0x51_u8; 32])?,
        founding_stake(1, pub_b.to_bytes(), [0x52_u8; 32])?,
    ];

    let bedrock_config = BedrockConfig {
        channel_id: config::bedrock_channel_id(),
        node_url: config::addr_to_url(config::UrlProtocol::Http, bedrock_addr)?,
        funding_key: config::bedrock_funding_key(),
        auth: None,
        priority_fee: sequencer_core::config::default_priority_fee(),
    };

    let partial = SequencerPartialConfig {
        block_create_timeout: Duration::from_secs(5),
        ..SequencerPartialConfig::default()
    };

    // Phase 1: A solo (it creates the channel), plus an indexer.
    let (seq_a, _a_home) = SequencerSetup::new(partial, bedrock_addr)
        .with_genesis(genesis.clone())
        .with_bedrock_signing_key(key_a)
        .setup()
        .await
        .context("Failed to set up sequencer A")?;
    let a = sequencer_client(seq_a.addr())?;
    let (idx, _idx_home) = setup_indexer(bedrock_addr, config::bedrock_channel_id(), None)
        .await
        .context("Failed to set up indexer")?;
    let indexer = indexer_client(idx.addr()).await?;

    wait_for_height(&a, 2, "sequencer A to produce past genesis").await?;

    // Phase 2: the creating tx carried the committee, so both keys are accredited
    // from block 1 and discovery has nothing to reconcile.
    let mut want = vec![pub_a.to_bytes(), pub_b.to_bytes()];
    want.sort_unstable();
    wait_until("Bedrock to accredit both staked keys", || async {
        Ok(committee(&bedrock_config).await?.0 == want)
    })
    .await?;

    // Phase 3: B joins live and syncs the existing chain.
    let (seq_b, _b_home) = SequencerSetup::new(partial, bedrock_addr)
        .with_genesis(genesis)
        .with_bedrock_signing_key(key_b)
        .setup()
        .await
        .context("Failed to set up sequencer B")?;
    let b = sequencer_client(seq_b.addr())?;

    let join_height = a.get_last_block_id().await?;
    wait_for_height(&b, join_height, "B to sync to A's height at join").await?;

    // Phase 4: rotation + convergence over ≈4 turn windows. Without the turn
    // check, a chain A produces alone satisfies every assertion below.
    let rotation_target = join_height + ROTATION_BLOCKS;
    wait_for_height(
        &a,
        rotation_target,
        "the chain to advance across turn windows",
    )
    .await?;
    wait_for_height(&b, rotation_target, "B to follow across turn windows").await?;
    wait_until("the round-robin turn to reach B", || async {
        Ok(committee(&bedrock_config).await?.1 == Some(pub_b))
    })
    .await?;
    assert_same_chain(&a, &b).await?;

    // Phase 5: a tx submitted only to B is included by B and visible on A.
    let accounts = initial_public_user_accounts();
    let from = accounts[0].account_id;
    let to = accounts[1].account_id;
    let sign_key = initial_pub_accounts_private_keys()[0].pub_sign_key.clone();

    let to_balance_before = a.get_account_balance(to).await?;
    let nonce = b.get_accounts_nonces(vec![from]).await?[0];
    let tx = common::test_utils::create_transaction_native_token_transfer(
        from,
        nonce.0,
        to,
        TRANSFER_AMOUNT,
        &sign_key,
    );
    b.send_transaction(tx)
        .await
        .context("Failed to submit the transfer to B")?;

    let expected = to_balance_before + TRANSFER_AMOUNT;
    wait_until("the cross-sequencer transfer to reach A", || async {
        Ok(a.get_account_balance(to).await? == expected)
    })
    .await?;

    // Phase 6: the indexer finalizes the same chain, with no stall.
    wait_until("the indexer to finalize", || async {
        Ok(indexer.get_last_finalized_block_id().await?.unwrap_or(0) >= join_height)
    })
    .await?;
    let finalized = indexer.get_last_finalized_block_id().await?.unwrap_or(0);
    for id in 1..=finalized {
        let block_i = indexer
            .get_block_by_id(id)
            .await?
            .with_context(|| format!("Indexer is missing finalized block {id}"))?;
        let block_a = a
            .get_block(id)
            .await?
            .with_context(|| format!("A is missing block {id}"))?;
        ensure!(
            block_i.header.hash == indexer_service_protocol::HashType::from(block_a.header.hash),
            "Indexer diverges from A at block {id}"
        );
    }
    let status = indexer.get_status().await?;
    ensure!(
        status.stall_reason.is_none(),
        "Indexer is stalled: {:?}",
        status.stall_reason
    );

    Ok(())
}

/// Builds a founding-sequencer genesis entry, signing its stake the way an
/// operator would before handing the entry over.
fn founding_stake(
    index: usize,
    sequencer_key: sequencer_stake_core::SequencerKey,
    owner_seed: [u8; 32],
) -> Result<GenesisAction> {
    let owner = lee::PrivateKey::try_new(owner_seed)?;
    Ok(GenesisAction::StakeSequencer {
        sequencer_key,
        ownership_public_key: lee::PublicKey::new_from_private_key(&owner),
        stake_signature: sign_genesis_stake(index, sequencer_key, &owner),
    })
}

/// Polls `check` until it reports ready, failing with `what` on timeout.
async fn wait_until<F, Fut>(what: &str, mut check: F) -> Result<()>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<bool>>,
{
    let wait = async {
        while !check().await? {
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        Ok::<(), anyhow::Error>(())
    };
    tokio::time::timeout(PHASE_TIMEOUT, wait)
        .await
        .with_context(|| format!("Timed out waiting for {what}"))?
}

/// Polls the sequencer until its chain height reaches `target`.
async fn wait_for_height(client: &SequencerClient, target: u64, what: &str) -> Result<()> {
    wait_until(&format!("{what} (target height {target})"), || async {
        Ok(client.get_last_block_id().await? >= target)
    })
    .await
}

/// The channel's accredited keys, sorted, plus whose turn the tip was written on.
async fn committee(config: &BedrockConfig) -> Result<(Vec<[u8; 32]>, Option<Ed25519PublicKey>)> {
    let Some(state) = read_channel_state(config).await? else {
        return Ok((Vec::new(), None));
    };
    let turn = state
        .accredited_keys
        .get(usize::from(state.tip_sequencer))
        .copied();
    let mut keys: Vec<_> = state
        .accredited_keys
        .iter()
        .map(Ed25519PublicKey::to_bytes)
        .collect();
    keys.sort_unstable();
    Ok((keys, turn))
}

/// Asserts A and B hold byte-identical block hashes over their common prefix.
async fn assert_same_chain(a: &SequencerClient, b: &SequencerClient) -> Result<()> {
    let common = a
        .get_last_block_id()
        .await?
        .min(b.get_last_block_id().await?);
    for id in 1..=common {
        let block_a = a
            .get_block(id)
            .await?
            .with_context(|| format!("A is missing block {id}"))?;
        let block_b = b
            .get_block(id)
            .await?
            .with_context(|| format!("B is missing block {id}"))?;
        ensure!(
            block_a.header.hash == block_b.header.hash,
            "Chain divergence at block {id}: A {:?} vs B {:?}",
            block_a.header.hash,
            block_b.header.hash
        );
    }
    Ok(())
}
