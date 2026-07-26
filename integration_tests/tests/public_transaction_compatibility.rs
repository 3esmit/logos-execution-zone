#![expect(
    clippy::tests_outside_test_module,
    reason = "Integration tests use the crate root so Cargo can discover them"
)]

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Context as _, Result};
use common::transaction::LeeTransaction;
use integration_tests::{TIME_TO_WAIT_FOR_BLOCK_SECONDS, TestContext};
use jsonrpsee::{
    RpcModule,
    server::ServerBuilder,
    types::{ErrorObjectOwned, error::INTERNAL_ERROR_CODE},
};
use lee::{AccountId, program::Program};
use sequencer_service_rpc::{RpcClient as _, SequencerClient};
use test_fixtures::{config::default_public_accounts_for_wallet, setup::setup_wallet};
use wallet::{AccountIdentity, config::WalletConfigOverrides};

const TRANSACTION_APPEAR_TIMEOUT: Duration =
    Duration::from_secs(2 * TIME_TO_WAIT_FOR_BLOCK_SECONDS);
const TRANSACTION_POLL_INTERVAL: Duration = Duration::from_millis(500);

struct LegacyCompatibleProxy {
    upstream: SequencerClient,
    send_transaction_calls: Arc<AtomicUsize>,
}

fn rpc_error(error: impl std::fmt::Display) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(INTERNAL_ERROR_CODE, error.to_string(), None::<()>)
}

async fn start_legacy_compatible_proxy(
    upstream: SequencerClient,
) -> Result<(
    jsonrpsee::server::ServerHandle,
    std::net::SocketAddr,
    Arc<AtomicUsize>,
)> {
    let send_transaction_calls = Arc::new(AtomicUsize::new(0));
    let mut module = RpcModule::new(LegacyCompatibleProxy {
        upstream,
        send_transaction_calls: Arc::clone(&send_transaction_calls),
    });

    module.register_async_method("getLastBlockId", |_params, proxy_state, _| async move {
        proxy_state
            .upstream
            .get_last_block_id()
            .await
            .map_err(rpc_error)
    })?;
    module.register_async_method("getAccount", |params, proxy_state, _| async move {
        let account_id: AccountId = params.one()?;
        proxy_state
            .upstream
            .get_account(account_id)
            .await
            .map_err(rpc_error)
    })?;
    module.register_async_method("sendTransaction", |params, proxy_state, _| async move {
        let transaction: LeeTransaction = params.one()?;
        proxy_state
            .send_transaction_calls
            .fetch_add(1, Ordering::Relaxed);
        proxy_state
            .upstream
            .send_transaction(transaction)
            .await
            .map_err(rpc_error)
    })?;

    let server = ServerBuilder::default().build("127.0.0.1:0").await?;
    let address = server.local_addr()?;
    let handle = server.start(module);

    Ok((handle, address, send_transaction_calls))
}

#[tokio::test]
async fn public_transaction_reaches_send_transaction_without_bulk_proofs() -> Result<()> {
    let context = TestContext::builder().disable_indexer().build().await?;
    let (proxy, proxy_address, send_transaction_calls) =
        start_legacy_compatible_proxy(context.sequencer_client().clone()).await?;
    let initial_public_accounts = default_public_accounts_for_wallet();
    let (wallet, _wallet_home, _password) = setup_wallet(
        proxy_address,
        &initial_public_accounts,
        &[],
        WalletConfigOverrides::default(),
    )
    .await?;
    let accounts = context.existing_public_accounts();
    let instruction_data =
        Program::serialize_instruction(authenticated_transfer_core::Instruction::Transfer {
            amount: 100,
        })?;

    let transaction_hash = wallet
        .send_pub_tx(
            vec![
                AccountIdentity::Public(accounts[0]),
                AccountIdentity::Public(accounts[1]),
            ],
            instruction_data,
            programs::authenticated_transfer().id(),
        )
        .await?;
    assert_eq!(
        send_transaction_calls.load(Ordering::Relaxed),
        1,
        "public transaction should be submitted once"
    );

    tokio::time::timeout(TRANSACTION_APPEAR_TIMEOUT, async {
        loop {
            if context
                .sequencer_client()
                .get_transaction(transaction_hash)
                .await?
                .is_some()
            {
                return Ok::<(), anyhow::Error>(());
            }
            tokio::time::sleep(TRANSACTION_POLL_INTERVAL).await;
        }
    })
    .await
    .context("timed out waiting for the upstream sequencer to accept the public transaction")??;

    proxy.stop()?;
    proxy.stopped().await;
    Ok(())
}
