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
use sov_modules_api::{CryptoSpec, NodeEndpoints, Spec, ZkVerifier};
use sov_modules_rollup_blueprint::pluggable_traits::PluggableSpec;
use sov_modules_rollup_blueprint::proof_sender::SovApiProofSender;
use sov_modules_rollup_blueprint::{FullNodeBlueprint, RollupBlueprint, SequencerCreationReceipt};
use sov_risc0_adapter::host::Risc0Host;
use sov_risc0_adapter::{Risc0, Risc0CryptoSpec};
use sov_rollup_full_node_interface::StateUpdateReceiver;
use sov_rollup_interface::da::{DaSpec, DaVerifier};
use sov_rollup_interface::node::SyncStatus;
use sov_sequencer::ProofBlobSender;
use sov_state::nomt::prover_storage::NomtProverStorage;
use sov_state::{DefaultStorageSpec, Storage};
use sov_stf_runner::processes::{ParallelProverService, ProverService, RollupProverConfig};
use sov_stf_runner::RollupConfig;

/// Marker type for the Celestia / mock-zkVM rollup. Carries no
/// state; the [`Default`] / [`Clone`] / [`Copy`] impls let the
/// blueprint machinery construct it however it wants.
#[derive(Default, Clone, Copy, Debug)]
pub struct CelestiaLigateRollup<M> {
    phantom: std::marker::PhantomData<M>,
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

    fn create_outer_code_commitment(
        &self,
    ) -> <<Self::ProverService as ProverService>::Verifier as ZkVerifier>::CodeCommitment {
        // MockZkvm accepts any commitment. A real prover swaps this
        // for the production circuit commitment in Phase A.4.
        MockCodeCommitment::default()
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

        sov_modules_rollup_blueprint::register_endpoints::<Self, Native>(
            state_update_receiver,
            sync_status_receiver,
            shutdown_receiver,
            ledger_db,
            sequencer,
            rollup_config,
        )
        .await
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
    ) -> Self::ProverService {
        // Real Risc0 inner host pinned to our guest ELF; outer
        // stays Mock (matches the SDK demo's pattern — outer
        // aggregation can stay mock until cross-chain proof
        // aggregation matters). DA verification is fully real.
        //
        // Under `SKIP_GUEST_BUILD=1` the ELF is empty and the
        // prover host won't be able to generate real proofs —
        // fine for `cargo check` / CI, but a node started this
        // way must have `RollupProverConfig::Disabled`.
        let inner_vm = Risc0Host::new(ligate_prover_risc0::ROLLUP_ELF);
        let outer_vm = MockZkvmHost::new_non_blocking();
        let da_verifier = CelestiaVerifier::new(RollupParams {
            rollup_batch_namespace: ROLLUP_BATCH_NAMESPACE,
            rollup_proof_namespace: ROLLUP_PROOF_NAMESPACE,
        });

        ParallelProverService::new_with_default_workers(
            inner_vm,
            outer_vm,
            da_verifier,
            rollup_config.proof_manager.prover_address,
        )
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
