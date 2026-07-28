//! Shared test/bench fixtures: spins up bedrock + sequencer + indexer + wallet
//! end-to-end against docker-compose, exposes a `TestContext` callers can drive.

use std::{collections::HashMap, net::SocketAddr, path::Path, sync::LazyLock};

use anyhow::{Context as _, Result};
use common::{HashType, transaction::LeeTransaction};
use futures::FutureExt as _;
use indexer_service::{ChannelId, IndexerHandle};
use lee::{AccountId, PrivacyPreservingTransaction};
use lee_core::Commitment;
use log::{debug, error};
use sequencer_core::config::GenesisAction;
use sequencer_service::SequencerHandle;
use sequencer_service_rpc::{RpcClient as _, SequencerClient};
use serde::Serialize;
use tempfile::TempDir;
use testcontainers::compose::DockerCompose;
use wallet::{
    WalletCore, account::AccountIdWithPrivacy, cli::CliAccountMention,
    config::WalletConfigOverrides,
};

use crate::{
    config::MultiNodeTestContextConfig,
    indexer_client::IndexerClient,
    setup::{
        SequencerSetup, setup_bedrock_node, setup_indexer,
        setup_private_accounts_with_initial_supply, setup_public_accounts_with_initial_supply,
        setup_wallet, sync_wallet_from_prebuilt,
    },
};

pub mod config;
pub mod indexer_client;
pub mod setup;

// TODO: Remove this and control time from tests
pub const TIME_TO_WAIT_FOR_BLOCK_SECONDS: u64 = 12;

pub(crate) const BEDROCK_SERVICE_WITH_OPEN_PORT: &str = "logos-blockchain-node-0";
pub(crate) const BEDROCK_SERVICE_PORT: u16 = 18080;

static LOGGER: LazyLock<()> = LazyLock::new(env_logger::init);

struct IndexerComponents {
    indexer_handle: IndexerHandle,
    indexer_client: IndexerClient,
    temp_dir: TempDir,
}

impl Drop for IndexerComponents {
    fn drop(&mut self) {
        let Self {
            indexer_handle,
            indexer_client: _,
            temp_dir: _,
        } = self;

        if !indexer_handle.is_healthy() {
            error!("Indexer handle has unexpectedly stopped before IndexerComponents drop");
        }
    }
}

/// Recursively-sized bytes on disk for sequencer / indexer / wallet tempdirs.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct DiskSizes {
    pub sequencer_bytes: u64,
    pub indexer_bytes: u64,
    pub wallet_bytes: u64,
}

pub struct SequencerComponent {
    /// In fact, not optional, just for Drop implementation.
    sequencer_handle: Option<SequencerHandle>,
    temp_sequencer_dir: TempDir,
    sequencer_client: SequencerClient,
}

pub struct TestContextZone {
    /// Every zone must have its own wallet, otherwise multi-sequecner client loses sence.
    wallet: WalletCore,
    wallet_password: String,
    temp_wallet_dir: TempDir,
    config: MultiNodeTestContextConfig,
    /// Each sequencer component is mapped from sequencer `SocketAddr`.
    sequencer_components: HashMap<SocketAddr, SequencerComponent>,
    indexer_components: Option<IndexerComponents>,
}

/// Test context which sets up a sequencer and a wallet for integration tests.
///
/// It's memory and logically safe to create multiple instances of this struct in parallel tests,
/// as each instance uses its own temporary directories for sequencer and wallet data.
// NOTE: Order of fields is important for proper drop order.
pub struct TestContext {
    zones: HashMap<ChannelId, TestContextZone>,
    bedrock_compose: DockerCompose,
    bedrock_addr: SocketAddr,
}

impl TestContext {
    /// Create new test context.
    pub async fn new_custom(configs: Vec<MultiNodeTestContextConfig>) -> Result<Self> {
        Self::builder(configs).build().await
    }

    /// Create new test context with default config(1 zone).
    pub async fn new() -> Result<Self> {
        Self::builder(vec![MultiNodeTestContextConfig::default()])
            .build()
            .await
    }

    /// Get a builder for the test context to customize its configuration.
    #[must_use]
    pub fn builder(configs: Vec<MultiNodeTestContextConfig>) -> MultiZoneTestContextBuilder {
        MultiZoneTestContextBuilder {
            zone_builders: configs.into_iter().map(TestContextBuilder::new).collect(),
        }
    }

    /// Reference for the default zone(in case if only one present).
    fn default_zone(&self) -> &TestContextZone {
        self.zones
            .values()
            .next()
            .expect("Must be at least one zone")
    }

    /// Reference for the default sequencer component(in case, if only one zone exists and only
    /// one sequencer exists).
    fn default_sequencer_component(&self) -> &SequencerComponent {
        self.default_zone()
            .sequencer_components
            .values()
            .next()
            .expect("Must be at least one integration component")
    }

    /// Mutable reference for the default zone(in case if only one present).
    fn default_zone_mut(&mut self) -> &mut TestContextZone {
        self.zones
            .values_mut()
            .next()
            .expect("Must be at least one zone")
    }

    /// Mutable reference for the default sequencer component(in case, if only one zone exists and
    /// only one sequencer exists).
    fn default_sequencer_component_mut(&mut self) -> &mut SequencerComponent {
        self.default_zone_mut()
            .sequencer_components
            .values_mut()
            .next()
            .expect("Must be at least one integration component")
    }

    /// Get reference to the deafault wallet.
    #[must_use]
    pub fn wallet(&self) -> &WalletCore {
        &self.default_zone().wallet
    }

    /// Get password of the default wallet.
    #[must_use]
    pub fn wallet_password(&self) -> &str {
        &self.default_zone().wallet_password
    }

    /// Get mutable reference to default the wallet.
    pub fn wallet_mut(&mut self) -> &mut WalletCore {
        &mut self.default_zone_mut().wallet
    }

    /// Get reference to the zone wallet.
    #[must_use]
    pub fn wallet_zone(&self, channel_id: ChannelId) -> Option<&WalletCore> {
        self.zones.get(&channel_id).map(|val| &val.wallet)
    }

    /// Get password of the zone wallet.
    #[must_use]
    pub fn wallet_password_zone(&self, channel_id: ChannelId) -> Option<&str> {
        self.zones
            .get(&channel_id)
            .map(|val| val.wallet_password.as_str())
    }

    /// Get mutable reference to the zone wallet.
    pub fn wallet_mut_zone(&mut self, channel_id: ChannelId) -> Option<&mut WalletCore> {
        self.zones.get_mut(&channel_id).map(|val| &mut val.wallet)
    }

    /// Get reference to the sequencer client in default case (1 zone, 1 sequencer).
    #[must_use]
    pub fn sequencer_client(&self) -> &SequencerClient {
        &self.default_sequencer_component().sequencer_client
    }

    /// Get reference to the sequencer client.
    #[must_use]
    pub fn sequencer_client_getter(
        &self,
        channel_id: ChannelId,
        addr: &SocketAddr,
    ) -> Option<&SequencerClient> {
        self.zones
            .get(&channel_id)
            .map(|val| {
                val.sequencer_components
                    .get(addr)
                    .map(|vall| &vall.sequencer_client)
            })
            .flatten()
    }

    /// Get the Bedrock Node address.
    #[must_use]
    pub const fn bedrock_addr(&self) -> SocketAddr {
        self.bedrock_addr
    }

    /// Get reference to the default indexer(1 zone).
    ///
    /// # Panics
    ///
    /// Panics if the indexer is not enabled in the test context. See
    /// [`TestContextBuilder::disable_indexer()`].
    #[must_use]
    pub fn indexer(&self) -> &IndexerHandle {
        &self
            .default_zone()
            .indexer_components
            .as_ref()
            .expect("Called `TestContext::indexer()` on context with disabled indexer")
            .indexer_handle
    }

    /// Get the default indexer's(1 zone) bound socket address.
    ///
    /// # Panics
    ///
    /// Panics if the indexer is not enabled in the test context.
    #[must_use]
    pub fn indexer_addr(&self) -> SocketAddr {
        self.indexer().addr()
    }

    /// Get reference to the default indexer(1 zone) client.
    ///
    /// # Panics
    ///
    /// Panics if the indexer is not enabled in the test context. See
    /// [`TestContextBuilder::disable_indexer()`].
    #[must_use]
    pub fn indexer_client(&self) -> &IndexerClient {
        &self
            .default_zone()
            .indexer_components
            .as_ref()
            .expect("Called `TestContext::indexer()` on context with disabled indexer")
            .indexer_client
    }

    /// Get reference to the indexer.
    ///
    /// # Panics
    ///
    /// Panics if the indexer is not enabled in the test context. See
    /// [`TestContextBuilder::disable_indexer()`].
    #[must_use]
    pub fn indexer_getter(&self, channel_id: ChannelId) -> Option<&IndexerHandle> {
        self.zones
            .get(&channel_id)
            .map(|val| {
                val.indexer_components
                    .as_ref()
                    .map(|val| &val.indexer_handle)
            })
            .flatten()
    }

    /// Get the default indexer's bound socket address.
    ///
    /// # Panics
    ///
    /// Panics if the indexer is not enabled in the test context.
    #[must_use]
    pub fn indexer_addr_getter(&self, channel_id: ChannelId) -> Option<SocketAddr> {
        self.indexer_getter(channel_id).map(|val| val.addr())
    }

    /// Get reference to the indexer client.
    ///
    /// # Panics
    ///
    /// Panics if the indexer is not enabled in the test context. See
    /// [`TestContextBuilder::disable_indexer()`].
    #[must_use]
    pub fn indexer_client_getter(&self, channel_id: ChannelId) -> Option<&IndexerClient> {
        self.zones
            .get(&channel_id)
            .map(|val| {
                val.indexer_components
                    .as_ref()
                    .map(|val| &val.indexer_client)
            })
            .flatten()
    }

    #[must_use]
    /// Get the multi-node config.
    pub fn config(&self, channel_id: ChannelId) -> Option<&MultiNodeTestContextConfig> {
        self.zones.get(&channel_id).map(|val| &val.config)
    }

    /// Recursively-sized bytes on disk for sequencer + indexer + wallet tempdirs.
    /// Indexer bytes are zero if the indexer is disabled.
    #[must_use]
    pub fn disk_sizes(&self) -> DiskSizes {
        DiskSizes {
            sequencer_bytes: self.zones.values().fold(0, |acc, zone| {
                acc.saturating_add(
                    zone.sequencer_components
                        .values()
                        .fold(0, |accc, component| {
                            accc.saturating_add(dir_size_bytes(component.temp_sequencer_dir.path()))
                        }),
                )
            }),
            indexer_bytes: self.zones.values().fold(0, |acc, zone| {
                acc.saturating_add(
                    zone.indexer_components
                        .as_ref()
                        .map_or(0, |val| dir_size_bytes(val.temp_dir.path())),
                )
            }),
            wallet_bytes: self.zones.values().fold(0, |acc, zone| {
                acc.saturating_add(dir_size_bytes(zone.temp_wallet_dir.path()))
            }),
        }
    }

    /// Get default(1 zone) existing public account IDs in the wallet.
    #[must_use]
    pub fn existing_public_accounts(&self) -> Vec<AccountId> {
        self.default_zone()
            .wallet
            .storage()
            .key_chain()
            .public_account_ids()
            .map(|(account_id, _idx)| account_id)
            .collect()
    }

    /// Get default(1 zone) existing private account IDs in the wallet.
    #[must_use]
    pub fn existing_private_accounts(&self) -> Vec<AccountId> {
        self.default_zone()
            .wallet
            .storage()
            .key_chain()
            .private_account_ids()
            .map(|(account_id, _idx)| account_id)
            .collect()
    }

    /// Get existing public account IDs in the wallet.
    #[must_use]
    pub fn existing_public_accounts_zone(&self, channel_id: ChannelId) -> Option<Vec<AccountId>> {
        self.wallet_zone(channel_id).map(|wallet_ref| {
            wallet_ref
                .storage()
                .key_chain()
                .public_account_ids()
                .map(|(account_id, _idx)| account_id)
                .collect()
        })
    }

    /// Get existing private account IDs in the wallet.
    #[must_use]
    pub fn existing_private_accounts_zone(&self, channel_id: ChannelId) -> Option<Vec<AccountId>> {
        self.wallet_zone(channel_id).map(|wallet_ref| {
            wallet_ref
                .storage()
                .key_chain()
                .private_account_ids()
                .map(|(account_id, _idx)| account_id)
                .collect()
        })
    }
}

impl Drop for TestContext {
    fn drop(&mut self) {
        let Self {
            zones,
            bedrock_compose,
            bedrock_addr: _,
        } = self;

        for TestContextZone {
            wallet: _,
            wallet_password: _,
            temp_wallet_dir: _,
            config: _,
            sequencer_components,
            indexer_components: _,
        } in zones.values_mut()
        {
            for SequencerComponent {
                sequencer_handle,
                temp_sequencer_dir: _,
                sequencer_client: _,
            } in sequencer_components.values_mut()
            {
                let sequencer_handle = sequencer_handle
                    .take()
                    .expect("Sequencer handle should be present in TestContext drop");

                if !sequencer_handle.is_healthy() {
                    let Err(err) = sequencer_handle
                        .failed()
                        .now_or_never()
                        .expect("Sequencer handle should not be running");
                    error!(
                        "Sequencer handle has unexpectedly stopped before TestContext drop with error: {err:#}"
                    );
                }
            }
        }

        let container = bedrock_compose
            .service(BEDROCK_SERVICE_WITH_OPEN_PORT)
            .unwrap_or_else(|| {
                panic!("Failed to get Bedrock service container `{BEDROCK_SERVICE_WITH_OPEN_PORT}`")
            });
        let output = std::process::Command::new("docker")
            .args(["inspect", "-f",  "{{.State.Running}}", container.id()])
            .output()
            .expect("Failed to execute docker inspect command to check if Bedrock container is still running");
        let stdout = String::from_utf8(output.stdout)
            .expect("Failed to parse docker inspect output as String");
        if stdout.trim() != "true" {
            error!(
                "Bedrock container `{}` is not running during TestContext drop, docker inspect output: {stdout}",
                container.id()
            );
        }
    }
}

pub struct TestContextBuilder {
    genesis_transactions: Option<Vec<GenesisAction>>,
    sequencer_partial_config: Option<config::SequencerPartialConfig>,
    enable_indexer: bool,
    wallet_config_overrides: WalletConfigOverrides,
    from_scratch: bool,
    config: MultiNodeTestContextConfig,
}

pub struct MultiZoneTestContextBuilder {
    zone_builders: Vec<TestContextBuilder>,
}

impl TestContextBuilder {
    fn new(config: MultiNodeTestContextConfig) -> Self {
        Self {
            genesis_transactions: None,
            sequencer_partial_config: None,
            enable_indexer: true,
            wallet_config_overrides: WalletConfigOverrides::default(),
            from_scratch: false,
            config,
        }
    }

    /// Override wallet config fields (e.g. polling timeouts) for the wallet built by this context.
    #[must_use]
    pub fn with_wallet_config_overrides(
        mut self,
        wallet_config_overrides: WalletConfigOverrides,
    ) -> Self {
        self.wallet_config_overrides = wallet_config_overrides;
        self
    }

    #[must_use]
    pub fn with_genesis(mut self, genesis_transactions: Vec<GenesisAction>) -> Self {
        self.genesis_transactions = Some(genesis_transactions);
        self
    }

    #[must_use]
    pub const fn with_sequencer_partial_config(
        mut self,
        sequencer_partial_config: config::SequencerPartialConfig,
    ) -> Self {
        self.sequencer_partial_config = Some(sequencer_partial_config);
        self
    }

    /// Build from genesis live instead of loading the prebuilt fixture. Implied by
    /// [`Self::with_genesis`].
    #[must_use]
    pub const fn from_scratch(mut self) -> Self {
        self.from_scratch = true;
        self
    }

    /// Exclude Indexer from test context.
    /// Indexer is enabled by default.
    ///
    /// Methods like [`TestContext::indexer()`] and [`TestContext::indexer_client()`] will panic if
    /// called when indexer is disabled.
    #[must_use]
    pub const fn disable_indexer(mut self) -> Self {
        self.enable_indexer = false;
        self
    }

    pub async fn build(self, bedrock_addr: SocketAddr) -> Result<TestContextZone> {
        let Self {
            genesis_transactions,
            sequencer_partial_config,
            enable_indexer,
            wallet_config_overrides,
            from_scratch,
            config,
        } = self;

        // Ensure logger is initialized only once
        *LOGGER;

        debug!("Test context setup");

        // The fixture bakes in the default accounts + genesis, so custom genesis / from_scratch
        // must build live. Otherwise load the fixture (fails if it is missing).
        let use_prebuilt = !from_scratch && genesis_transactions.is_none();

        let indexer_components = if enable_indexer {
            let (indexer_handle, temp_indexer_dir) =
                setup_indexer(bedrock_addr, config::bedrock_channel_id(), None)
                    .await
                    .context("Failed to setup Indexer")?;
            let indexer_client = setup::indexer_client(indexer_handle.addr())
                .await
                .context("Failed to create indexer client")?;
            Some(IndexerComponents {
                indexer_handle,
                indexer_client,
                temp_dir: temp_indexer_dir,
            })
        } else {
            None
        };

        let initial_public_accounts = config::default_public_accounts_for_wallet();
        let initial_private_accounts = config::default_private_accounts_for_wallet();

        let partial_config = sequencer_partial_config.unwrap_or_default();

        let mut sequencer_handles = vec![];
        let mut temp_sequencer_dirs = vec![];
        let mut sequencer_clients = HashMap::new();

        for _ in 0..config.num_nodes {
            let mut sequencer_setup = SequencerSetup::new(partial_config, bedrock_addr);
            if !use_prebuilt {
                // Wallet genesis must always be present so that
                // setup_public/private_accounts_with_initial_supply can claim from the vault PDAs.
                // When a test supplies custom genesis, merge rather than replace.
                let wallet_genesis = config::genesis_from_accounts(
                    &initial_public_accounts,
                    &initial_private_accounts,
                );
                let genesis = match genesis_transactions.clone() {
                    Some(mut custom) => {
                        custom.extend(wallet_genesis);
                        custom
                    }
                    None => wallet_genesis,
                };
                sequencer_setup = sequencer_setup.with_genesis(genesis);
            }
            let (sequencer_handle, temp_sequencer_dir) = sequencer_setup
                .setup()
                .await
                .context("Failed to setup Sequencer")?;

            let sequencer_client = setup::sequencer_client(sequencer_handle.addr())
                .context("Failed to create sequencer client")?;

            sequencer_clients.insert(sequencer_handle.addr(), sequencer_client);

            sequencer_handles.push(sequencer_handle);
            temp_sequencer_dirs.push(temp_sequencer_dir);
        }

        let (mut wallet, temp_wallet_dir, wallet_password) = setup_wallet(
            &sequencer_handles
                .iter()
                .map(sequencer_service::SequencerHandle::addr)
                .collect::<Vec<_>>(),
            &initial_public_accounts,
            &initial_private_accounts,
            wallet_config_overrides,
        )
        .await
        .context("Failed to setup wallet")?;

        if use_prebuilt {
            // Funds already exist on-chain in the prebuilt blocks; sync instead of claiming live.
            sync_wallet_from_prebuilt(&mut wallet)
                .await
                .context("Failed to sync wallet from prebuilt database")?;
        } else {
            setup_public_accounts_with_initial_supply(&mut wallet, &initial_public_accounts)
                .await
                .context("Failed to initialize public accounts in wallet")?;

            setup_private_accounts_with_initial_supply(&mut wallet, &initial_private_accounts)
                .await
                .context("Failed to initialize private accounts in wallet")?;
        }

        Ok(TestContext {
            sequencer_clients,
            wallet,
            wallet_password,
            bedrock_compose,
            bedrock_addr,
            sequencer_handles: Some(sequencer_handles),
            indexer_components,
            temp_sequencer_dirs,
            temp_wallet_dir,
            config,
        })
    }

    pub fn build_blocking(self) -> Result<BlockingTestContext> {
        let runtime = tokio::runtime::Runtime::new().context("Failed to create Tokio runtime")?;

        let ctx = runtime.block_on(self.build())?;

        Ok(BlockingTestContext {
            ctx: Some(ctx),
            runtime,
        })
    }
}

impl MultiZoneTestContextBuilder {
    pub async fn build(self) -> Result<TestContext> {
        let (bedrock_compose, bedrock_addr) = setup_bedrock_node()
            .await
            .context("Failed to setup Bedrock node")?;

        self.zone_builders
            .into_iter()
            .map(|ctxb| TestContextBuilder::build(ctxb, bedrock_addr.clone()))
    }
}

/// A test context to be used in normal #[test] tests.
pub struct BlockingTestContext {
    ctx: Option<TestContext>,
    runtime: tokio::runtime::Runtime,
}

impl BlockingTestContext {
    pub fn new(config: MultiNodeTestContextConfig) -> Result<Self> {
        TestContext::builder(config).build_blocking()
    }

    pub const fn ctx(&self) -> &TestContext {
        self.ctx.as_ref().expect("TestContext is set")
    }

    pub const fn ctx_mut(&mut self) -> &mut TestContext {
        self.ctx.as_mut().expect("TestContext is set")
    }

    pub const fn runtime(&self) -> &tokio::runtime::Runtime {
        &self.runtime
    }

    pub fn block_on<'ctx, F>(&'ctx self, f: impl FnOnce(&'ctx TestContext) -> F) -> F::Output
    where
        F: std::future::Future + 'ctx,
    {
        let future = f(self.ctx());
        self.runtime.block_on(future)
    }

    pub fn block_on_mut<'ctx, F>(
        &'ctx mut self,
        f: impl FnOnce(&'ctx mut TestContext) -> F,
    ) -> F::Output
    where
        F: std::future::Future + 'ctx,
    {
        let ctx_mut = self.ctx.as_mut().expect("TestContext is set");
        let future = f(ctx_mut);
        self.runtime.block_on(future)
    }
}

impl Drop for BlockingTestContext {
    fn drop(&mut self) {
        let Self { ctx, runtime } = self;

        // Ensure async cleanup of TestContext by blocking on its drop in the runtime.
        runtime.block_on(async {
            if let Some(ctx) = ctx.take() {
                drop(ctx);
            }
        });
    }
}

#[must_use]
pub const fn public_mention(account_id: AccountId) -> CliAccountMention {
    CliAccountMention::Id(AccountIdWithPrivacy::Public(account_id))
}

#[must_use]
pub const fn private_mention(account_id: AccountId) -> CliAccountMention {
    CliAccountMention::Id(AccountIdWithPrivacy::Private(account_id))
}

#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "We want the code to panic if the transaction type is not PrivacyPreserving"
)]
pub async fn fetch_privacy_preserving_tx(
    seq_client: &SequencerClient,
    tx_hash: HashType,
) -> PrivacyPreservingTransaction {
    let (tx, _block_id) = seq_client.get_transaction(tx_hash).await.unwrap().unwrap();

    match tx {
        LeeTransaction::PrivacyPreserving(privacy_preserving_transaction) => {
            privacy_preserving_transaction
        }
        _ => panic!("Invalid tx type"),
    }
}

pub async fn verify_commitment_is_in_state(
    commitment: Commitment,
    seq_client: &SequencerClient,
) -> bool {
    seq_client
        .get_proofs_and_root(vec![commitment])
        .await
        .ok()
        .and_then(|(proofs, _)| proofs.into_iter().next().flatten())
        .is_some()
}

fn dir_size_bytes(path: &Path) -> u64 {
    let mut total = 0_u64;
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_file() {
            total = total.saturating_add(metadata.len());
        } else if metadata.is_dir() {
            total = total.saturating_add(dir_size_bytes(&entry.path()));
        } else {
            // Sockets, FIFOs, block/char devices: ignore. Symlinks are
            // already followed by `is_file()` / `is_dir()`.
        }
    }
    total
}
