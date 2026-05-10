//! `RollupBlueprint` + `FullNodeBlueprint` impls for the mock-DA /
//! mock-zkVM flavour of Ligate Chain.
//!
//! Every method body delegates to the SDK's stock implementations.
//! Parity with the SDK demo's `mock_rollup.rs` is intentional — when
//! we eventually swap MockDa for Celestia (Phase A.3) the diff
//! should be limited to associated types + the `create_da_service`
//! body, not the structural shape.
//!
//! Two specific deviations from the SDK demo:
//!
//! 1. **No EVM RPC, no Solana router.** The demo's
//!    `sequencer_additional_apis` wires both. We inherit
//!    `FullNodeBlueprint`'s default impl (empty endpoints). Standard
//!    sov RPCs (sequencer, ledger, runtime modules) still register
//!    via `create_endpoints`.
//! 2. **Address type is [`MultiAddressEvm`]**, not the demo's
//!    `MultiAddressEvmSolana`. We don't need the Solana variant —
//!    the runtime's `S::Address` bound is `FromVmAddress<EthereumAddress>`
//!    only.

use std::sync::Arc;

use async_trait::async_trait;
use ligate_stf::Runtime;
use sov_address::{EthereumAddress, FromVmAddress, MultiAddressEvm};
use sov_db::ledger_db::LedgerDb;
use sov_db::storage_manager::NomtStorageManager;
use sov_mock_da::storable::StorableMockDaService;
use sov_mock_da::MockDaSpec;
use sov_mock_zkvm::{MockCodeCommitment, MockZkvm, MockZkvmCryptoSpec, MockZkvmHost};
use sov_modules_api::configurable_spec::ConfigurableSpec;
use sov_modules_api::execution_mode::Native;
use sov_modules_api::{CryptoSpec, NodeEndpoints, Spec};
use sov_modules_rollup_blueprint::pluggable_traits::PluggableSpec;
use sov_modules_rollup_blueprint::proof_sender::SovApiProofSender;
use sov_modules_rollup_blueprint::{FullNodeBlueprint, RollupBlueprint, SequencerCreationReceipt};
use sov_rollup_full_node_interface::StateUpdateReceiver;
use sov_rollup_interface::da::DaSpec;
use sov_rollup_interface::node::SyncStatus;
use sov_sequencer::ProofBlobSender;
use sov_state::nomt::prover_storage::NomtProverStorage;
use sov_state::{DefaultStorageSpec, Storage};
use sov_stf_runner::processes::{ParallelProverService, RollupProverConfig};
use sov_stf_runner::RollupConfig;

/// Marker type for the mock-DA / mock-zkVM rollup.
///
/// Holds the configured `chain_id` so the blueprint's `create_endpoints`
/// hook can mount `/v1/rollup/info` with the operator-supplied
/// identifier (#181), and the [`crate::NodeRole`] so the `create_endpoints`
/// hook can layer the [`crate::follower_guard`] middleware when
/// running as a follower (#243). The [`Default`] impl is kept so
/// existing tests can construct the type without plumbing chain
/// config; production code paths (`main.rs`) pass the loaded id and
/// role via [`MockLigateRollup::new`].
#[derive(Clone, Debug)]
pub struct MockLigateRollup<M> {
    chain_id: std::sync::Arc<str>,
    node_role: crate::NodeRole,
    phantom: std::marker::PhantomData<M>,
}

impl<M> Default for MockLigateRollup<M> {
    fn default() -> Self {
        // Empty-string default is intentional. Tests using
        // [`MockLigateRollup::default`] don't exercise
        // `create_endpoints`, and a panicky `unwrap` in `Default`
        // would surface during macro-generated test compilation
        // before any meaningful diagnostic.
        Self {
            chain_id: std::sync::Arc::from(""),
            node_role: crate::NodeRole::Sequencer,
            phantom: std::marker::PhantomData,
        }
    }
}

impl<M> MockLigateRollup<M> {
    /// Build a blueprint with the configured chain id and node role.
    /// Used by `main.rs` after [`crate::chain_config::load_split_config`]
    /// reads + validates the `[chain]` section. Defaults to
    /// [`crate::NodeRole::Sequencer`] for backward-compat callers.
    pub fn new(chain_id: impl Into<std::sync::Arc<str>>) -> Self {
        Self {
            chain_id: chain_id.into(),
            node_role: crate::NodeRole::Sequencer,
            phantom: std::marker::PhantomData,
        }
    }

    /// Build a blueprint with both chain id and node role.
    pub fn new_with_role(
        chain_id: impl Into<std::sync::Arc<str>>,
        node_role: crate::NodeRole,
    ) -> Self {
        Self { chain_id: chain_id.into(), node_role, phantom: std::marker::PhantomData }
    }
}

type Hasher = <MockZkvmCryptoSpec as CryptoSpec>::Hasher;
type NativeStorage =
    NomtProverStorage<DefaultStorageSpec<Hasher>, <MockDaSpec as DaSpec>::SlotHash>;

/// Concrete [`Spec`] used by [`MockLigateRollup`].
///
/// `MultiAddressEvm` carries both 28-byte standard rollup addresses
/// and 20-byte Ethereum addresses. We don't ship Solidity execution
/// (the runtime omits `sov-evm`), but the address shape is still EVM-
/// aware so wallet UX maps cleanly to `0x…` and so the bank/accounts
/// modules' `FromVmAddress` bound is satisfied.
pub type MockRollupSpec<M> = ConfigurableSpec<
    MockDaSpec,
    MockZkvm,
    MockZkvm,
    MultiAddressEvm,
    M,
    MockZkvmCryptoSpec,
    NativeStorage,
>;

impl RollupBlueprint<Native> for MockLigateRollup<Native>
where
    MockRollupSpec<Native>: PluggableSpec,
    <MockRollupSpec<Native> as Spec>::Address: FromVmAddress<EthereumAddress>,
{
    type Spec = MockRollupSpec<Native>;
    type Runtime = Runtime<Self::Spec>;
}

#[async_trait]
impl FullNodeBlueprint<Native> for MockLigateRollup<Native> {
    type DaService = StorableMockDaService;

    type StorageManager = NomtStorageManager<MockDaSpec, Hasher, NativeStorage>;

    type ProverService = ParallelProverService<
        <Self::Spec as Spec>::Address,
        <<Self::Spec as Spec>::Storage as Storage>::Root,
        <<Self::Spec as Spec>::Storage as Storage>::Witness,
        Self::DaService,
        <Self::Spec as Spec>::InnerZkvm,
        <Self::Spec as Spec>::OuterZkvm,
    >;

    type ProofSender = SovApiProofSender<Self::Spec>;

    fn compute_code_commitments() -> anyhow::Result<(
        sov_modules_api::CodeCommitmentFor<<Self::Spec as Spec>::InnerZkvm>,
        sov_modules_api::CodeCommitmentFor<<Self::Spec as Spec>::OuterZkvm>,
    )> {
        // MockZkvm accepts any commitment as valid; defaults are fine
        // for both inner (state-transition) and outer (aggregation) circuits.
        Ok((MockCodeCommitment::default(), MockCodeCommitment::default()))
    }

    async fn create_endpoints(
        &self,
        state_update_receiver: StateUpdateReceiver<<Self::Spec as Spec>::Storage>,
        sync_status_receiver: tokio::sync::watch::Receiver<SyncStatus>,
        shutdown_receiver: tokio::sync::watch::Receiver<()>,
        ledger_db: &LedgerDb,
        sequencer: &SequencerCreationReceipt<Self::Spec>,
        _da_service: &Self::DaService,
        rollup_config: &RollupConfig<<Self::Spec as Spec>::Address, Self::DaService>,
    ) -> anyhow::Result<NodeEndpoints> {
        // Phase 2 of #110: spawn the block-height polling task here
        // because `LedgerDb` is only handed to us via this hook.
        // The task lives until the tokio runtime tears down.
        crate::metrics::spawn_block_height_task(
            ledger_db.clone(),
            crate::metrics::DEFAULT_BLOCK_HEIGHT_POLL_INTERVAL,
        );

        // Phase 6.1 of #110: spawn the mempool-depth polling task.
        // `mempool_metrics` is exposed on `SequencerCreationReceipt`
        // by our `ligate-mainline` SDK fork patch (#164 was blocked on
        // upstream Sovereign exposing this; our fork unblocks it).
        crate::metrics::spawn_mempool_depth_task(
            sequencer.mempool_metrics.clone(),
            crate::metrics::DEFAULT_MEMPOOL_DEPTH_POLL_INTERVAL,
        );

        // Phase 6.2 of #110: subscribe to the BlobSender's broadcast
        // channel for DA submission failures (by reason) and DA
        // finalization latency (Published -> Finalized). Channel
        // exposure also comes from the `ligate-mainline` fork patch.
        crate::metrics::spawn_da_metrics_task(sequencer.blob_status_channel.clone());

        // Pre-touch the RPC counter + histogram so their HELP/TYPE
        // lines appear from the first /metrics scrape.
        crate::metrics::init_rpc_metrics();

        // Process-level metrics (CPU / RSS / FDs) via prometheus's
        // `process` feature. Idempotent registration.
        crate::metrics::register_process_collector();

        // Clone sync_status_receiver before passing it into the SDK
        // helper. The clone is a watch::Receiver alias to the same
        // channel and is what feeds the /ready endpoint below.
        let sync_status_for_health = sync_status_receiver.clone();

        // Stock SDK helper. Wires the standard sequencer + ledger
        // RPCs and the runtime's per-module REST routers (currently
        // empty, see `ligate_stf::runtime_capabilities`).
        let mut endpoints = sov_modules_rollup_blueprint::register_endpoints::<Self, Native>(
            state_update_receiver,
            sync_status_receiver,
            shutdown_receiver,
            ledger_db,
            sequencer,
            rollup_config,
        )
        .await?;

        // #149: nest the SDK-mounted chain API under `/v1/` so
        // future breaking changes can land at `/v2/...` without
        // colliding with the existing surface. Pre-public-devnet
        // is the right time to lock the URL convention; post-
        // mainnet a rename costs migration windows.
        //
        // #181: merge `/rollup/info` into the same `/v1` group so it
        // sits next to the SDK's `/rollup/sync-status`,
        // `/rollup/constants`, etc. The handler reads
        // `chain_id` from the blueprint's stored value (loaded from
        // `[chain]`) and the runtime's build-time `CHAIN_HASH`.
        let chain_api = std::mem::take::<axum::Router<()>>(&mut endpoints.axum_router);
        let info_state = crate::info::InfoState::new(
            self.chain_id.clone(),
            <Self::Runtime as sov_modules_api::Runtime<Self::Spec>>::CHAIN_HASH,
        );
        let chain_api = crate::info::add_routes(chain_api, info_state);
        let mut router = axum::Router::new().nest("/v1", chain_api);

        // #176: mount /health + /ready at root, NOT under /v1.
        // Operator probes don't change schema across API versions
        // and load balancers / k8s expect them at conventional
        // unversioned paths.
        let health_state = crate::health::HealthState::new(sync_status_for_health);
        router = crate::health::add_routes(router, health_state);

        // Phase 2 of #110: layer the Prometheus middleware over the
        // outer router (after /v1 nest + health routes) so metric
        // labels see the full path. /v1/* routes show with the
        // version prefix; /health and /ready show unprefixed.
        // `MatchedPath::as_str()` keeps cardinality bounded by
        // route template, not concrete `:id`.
        router = router.layer(axum::middleware::from_fn(crate::metrics::record_rpc_request));

        // #243: in follower mode, return 503 on POST /v1/sequencer/txs
        // so submissions don't silently disappear into a local mempool
        // that never propagates. In sequencer mode the layer is not
        // applied at all (zero overhead for the common case).
        if self.node_role == crate::NodeRole::Follower {
            router = router.layer(axum::middleware::from_fn(
                crate::follower_guard::block_sequencer_submission,
            ));
        }

        endpoints.axum_router = router;

        Ok(endpoints)
    }

    async fn create_da_service(
        &self,
        rollup_config: &RollupConfig<<Self::Spec as Spec>::Address, Self::DaService>,
        shutdown_receiver: tokio::sync::watch::Receiver<()>,
    ) -> Self::DaService {
        StorableMockDaService::from_config(rollup_config.da.clone(), shutdown_receiver).await
    }

    async fn create_prover_service(
        &self,
        _prover_config: RollupProverConfig,
        rollup_config: &RollupConfig<<Self::Spec as Spec>::Address, Self::DaService>,
        _da_service: &Self::DaService,
        _ledger_db: &LedgerDb,
        _start_fresh_outer_proof_on_resync: bool,
    ) -> (Self::ProverService, Option<sov_rollup_interface::common::SlotNumber>) {
        // Mock-zkVM devnet: blueprint always needs a ProverService instance
        // even when proving is disabled (`SOV_PROVER_MODE` unset).
        let inner_vm = MockZkvmHost::new_non_blocking();
        let outer_vm = MockZkvmHost::new_non_blocking();
        let da_verifier = Default::default();

        let prover = ParallelProverService::new_with_default_workers(
            inner_vm,
            outer_vm,
            da_verifier,
            rollup_config.proof_manager.prover_address,
            5,
        );
        (prover, None)
    }

    fn create_storage_manager(
        &self,
        rollup_config: &RollupConfig<<Self::Spec as Spec>::Address, Self::DaService>,
        witness_generation: bool,
    ) -> anyhow::Result<Self::StorageManager> {
        NomtStorageManager::new(rollup_config.storage.clone(), witness_generation)
    }

    fn create_proof_sender(
        &self,
        _rollup_config: &RollupConfig<<Self::Spec as Spec>::Address, Self::DaService>,
        sequence_number_provider: Arc<dyn ProofBlobSender>,
    ) -> anyhow::Result<Self::ProofSender> {
        Ok(Self::ProofSender::new(sequence_number_provider))
    }
}
