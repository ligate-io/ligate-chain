//! `RollupBlueprint` + `FullNodeBlueprint` impls for the Celestia
//! flavour of Ligate Chain.
//!
//! Mirrors the SDK demo's `celestia_rollup.rs` with one intentional
//! deviation: no EVM RPC, no Solana router. The demo's
//! `sequencer_additional_apis` wires both; we inherit
//! `FullNodeBlueprint`'s default impl (empty endpoints). Standard
//! sov RPCs (sequencer, ledger, runtime modules) still register
//! via `create_endpoints`.
//!
//! Inner zkVM is `Risc0`; outer stays `MockZkvm` (matches the SDK
//! demo's pattern — outer aggregation can stay mock until cross-
//! chain proof aggregation matters). The Risc0 guest binary lives
//! in [`ligate_prover_risc0`] and is consumed via `ROLLUP_ELF`.

use std::sync::Arc;

use async_trait::async_trait;
use ligate_stf::Runtime;
use sov_address::{EthereumAddress, FromVmAddress, MultiAddressEvm};
use sov_celestia_adapter::verifier::{CelestiaSpec, CelestiaVerifier, RollupParams};
use sov_celestia_adapter::CelestiaService;
use sov_db::ledger_db::LedgerDb;
use sov_db::storage_manager::NomtStorageManager;
use sov_mock_zkvm::{MockCodeCommitment, MockZkvm, MockZkvmHost};
use sov_modules_api::configurable_spec::ConfigurableSpec;
use sov_modules_api::execution_mode::Native;
use sov_modules_api::macros::config_value;
use sov_modules_api::{CryptoSpec, NodeEndpoints, Spec};
use sov_modules_rollup_blueprint::pluggable_traits::PluggableSpec;
use sov_modules_rollup_blueprint::proof_sender::SovApiProofSender;
use sov_modules_rollup_blueprint::{FullNodeBlueprint, RollupBlueprint, SequencerCreationReceipt};
use sov_risc0_adapter::host::Risc0Host;
use sov_risc0_adapter::{Risc0, Risc0CryptoSpec, Risc0MethodId};
use sov_rollup_full_node_interface::StateUpdateReceiver;
use sov_rollup_interface::da::{DaSpec, DaVerifier};
use sov_rollup_interface::node::SyncStatus;
use sov_sequencer::ProofBlobSender;
use sov_state::nomt::prover_storage::NomtProverStorage;
use sov_state::{DefaultStorageSpec, Storage};
use sov_stf_runner::processes::{ParallelProverService, RollupProverConfig};
use sov_stf_runner::RollupConfig;

/// Marker type for the Celestia / mock-zkVM rollup.
///
/// Same shape as [`crate::MockLigateRollup`] (#181): holds the
/// configured `chain_id` so `create_endpoints` can mount
/// `/v1/rollup/info` with the operator-supplied identifier. The
/// [`Default`] impl is preserved for tests; production
/// (`main.rs`) uses [`CelestiaLigateRollup::new`] after
/// [`crate::chain_config::load_split_config`] reads the `[chain]`
/// section.
#[derive(Clone, Debug)]
pub struct CelestiaLigateRollup<M> {
    chain_id: std::sync::Arc<str>,
    node_role: crate::NodeRole,
    phantom: std::marker::PhantomData<M>,
}

impl<M> Default for CelestiaLigateRollup<M> {
    fn default() -> Self {
        Self {
            chain_id: std::sync::Arc::from(""),
            node_role: crate::NodeRole::Sequencer,
            phantom: std::marker::PhantomData,
        }
    }
}

impl<M> CelestiaLigateRollup<M> {
    /// Build a blueprint with the configured chain id. Used by
    /// `main.rs` after `[chain]` is loaded and validated. Defaults to
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

type Hasher = <Risc0CryptoSpec as CryptoSpec>::Hasher;
type NativeStorage =
    NomtProverStorage<DefaultStorageSpec<Hasher>, <CelestiaSpec as DaSpec>::SlotHash>;

/// Concrete [`Spec`] used by [`CelestiaLigateRollup`].
///
/// Identical address shape to [`crate::MockRollupSpec`]
/// (`MultiAddressEvm` → 28-byte standard + 20-byte Ethereum). Two
/// things differ from the mock-DA spec:
///
/// 1. DA layer: `CelestiaSpec` instead of `MockDaSpec`.
/// 2. Inner zkVM: `Risc0` instead of `MockZkvm`. The host pins this
///    to [`ligate_prover_risc0::ROLLUP_ELF`] in
///    `create_prover_service` below; the guest binary at that
///    constant runs the same STF blueprint inside the zkVM.
pub type CelestiaRollupSpec<M> = ConfigurableSpec<
    CelestiaSpec,
    Risc0,
    MockZkvm,
    MultiAddressEvm,
    M,
    Risc0CryptoSpec,
    NativeStorage,
>;

/// Celestia namespace where the rollup posts its tx batches. Sourced
/// from `BATCH_NAMESPACE` in `constants.toml` so a misconfigured
/// node and the canonical chain can never disagree silently.
pub const ROLLUP_BATCH_NAMESPACE: sov_celestia_adapter::types::Namespace =
    sov_celestia_adapter::types::Namespace::const_v0(config_value!("BATCH_NAMESPACE"));

/// Celestia namespace where the rollup posts its zk proofs. Sourced
/// from `PROOF_NAMESPACE` in `constants.toml`.
pub const ROLLUP_PROOF_NAMESPACE: sov_celestia_adapter::types::Namespace =
    sov_celestia_adapter::types::Namespace::const_v0(config_value!("PROOF_NAMESPACE"));

impl RollupBlueprint<Native> for CelestiaLigateRollup<Native>
where
    CelestiaRollupSpec<Native>: PluggableSpec,
    <CelestiaRollupSpec<Native> as Spec>::Address: FromVmAddress<EthereumAddress>,
{
    type Spec = CelestiaRollupSpec<Native>;
    type Runtime = Runtime<Self::Spec>;
}

#[async_trait]
impl FullNodeBlueprint<Native> for CelestiaLigateRollup<Native> {
    type DaService = CelestiaService;

    type StorageManager = NomtStorageManager<CelestiaSpec, Hasher, NativeStorage>;

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
        // Inner Risc0 commitment derived from `ROLLUP_ID` ([u32; 8] from
        // risc0-build) via the chain's `CodeCommitmentTrait`. Outer stays
        // MockZkvm-default. Phase A.4 swaps the outer to a real commitment.
        use sov_rollup_interface::zk::aggregated_proof::CodeCommitmentHash;
        use sov_rollup_interface::zk::CodeCommitmentTrait;
        let inner = <Risc0MethodId as CodeCommitmentTrait>::from_hash(
            CodeCommitmentHash::from_u32_array(ligate_prover_risc0::ROLLUP_ID),
        );
        Ok((inner, MockCodeCommitment::default()))
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
        // Phase 2 of #110: same hook the mock-DA blueprint uses to
        // spawn the block-height polling task. `LedgerDb` is only
        // handed to us here.
        crate::metrics::spawn_block_height_task(
            ledger_db.clone(),
            crate::metrics::DEFAULT_BLOCK_HEIGHT_POLL_INTERVAL,
        );
        // Phase 6.1 of #110: mempool depth via the SDK fork's
        // `MempoolMetrics` exposure on the receipt.
        crate::metrics::spawn_mempool_depth_task(
            sequencer.mempool_metrics.clone(),
            crate::metrics::DEFAULT_MEMPOOL_DEPTH_POLL_INTERVAL,
        );
        crate::metrics::init_rpc_metrics();
        // Process-level metrics (CPU / RSS / FDs).
        crate::metrics::register_process_collector();

        let sync_status_for_health = sync_status_receiver.clone();

        let mut endpoints = sov_modules_rollup_blueprint::register_endpoints::<Self, Native>(
            state_update_receiver,
            sync_status_receiver,
            shutdown_receiver,
            ledger_db,
            sequencer,
            rollup_config,
        )
        .await?;

        // #149: nest chain API under /v1/. #176: /health + /ready
        // at root. #110: metrics middleware on the outer router.
        // #181: merge /rollup/info into the /v1 group, fed by the
        // configured chain_id and the runtime's CHAIN_HASH.
        let chain_api = std::mem::take::<axum::Router<()>>(&mut endpoints.axum_router);
        let info_state = crate::info::InfoState::new(
            self.chain_id.clone(),
            <Self::Runtime as sov_modules_api::Runtime<Self::Spec>>::CHAIN_HASH,
        );
        let chain_api = crate::info::add_routes(chain_api, info_state);
        let mut router = axum::Router::new().nest("/v1", chain_api);

        let health_state = crate::health::HealthState::new(sync_status_for_health);
        router = crate::health::add_routes(router, health_state);

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
        CelestiaService::new(
            rollup_config.da.clone(),
            RollupParams {
                rollup_batch_namespace: ROLLUP_BATCH_NAMESPACE,
                rollup_proof_namespace: ROLLUP_PROOF_NAMESPACE,
            },
            shutdown_receiver,
        )
        .await
    }

    async fn create_prover_service(
        &self,
        _prover_config: RollupProverConfig,
        rollup_config: &RollupConfig<<Self::Spec as Spec>::Address, Self::DaService>,
        _da_service: &Self::DaService,
        _ledger_db: &LedgerDb,
        _start_fresh_outer_proof_on_resync: bool,
    ) -> (Self::ProverService, Option<sov_rollup_interface::common::SlotNumber>) {
        // Real Risc0 inner host pinned to our guest ELF; outer
        // stays Mock (matches the SDK demo's pattern; outer
        // aggregation can stay mock until cross-chain proof
        // aggregation matters). DA verification is fully real.
        //
        // Under `SKIP_GUEST_BUILD=1` the ELF is empty and the
        // prover host won't be able to generate real proofs.
        // Fine for `cargo check` / CI, but a node started this
        // way must have `RollupProverConfig::Disabled`.
        let inner_vm = Risc0Host::new(ligate_prover_risc0::ROLLUP_ELF);
        let outer_vm = MockZkvmHost::new_non_blocking();
        let da_verifier = CelestiaVerifier::new(RollupParams {
            rollup_batch_namespace: ROLLUP_BATCH_NAMESPACE,
            rollup_proof_namespace: ROLLUP_PROOF_NAMESPACE,
        });

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
