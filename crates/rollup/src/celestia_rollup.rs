//! `RollupBlueprint` + `FullNodeBlueprint` impls for the Celestia
//! flavour of Ligate Chain.
//!
//! Mirrors the SDK demo's `celestia_rollup.rs` with two intentional
//! deviations:
//!
//! 1. **`MockZkvm` for both inner and outer.** The SDK demo uses
//!    Risc0 as its inner zkVM. We don't yet — Phase A.4 swaps in a
//!    real prover. Until then the chain runs against MockZkvm even
//!    on Celestia: real DA finality, mock proving.
//! 2. **No EVM RPC, no Solana router.** The demo's
//!    `sequencer_additional_apis` wires both. We inherit
//!    `FullNodeBlueprint`'s default impl (empty endpoints). Standard
//!    sov RPCs (sequencer, ledger, runtime modules) still register
//!    via `create_endpoints`.
//!
//! When Phase A.4 lands, the diff to switch to Risc0 will be
//! contained to the `Spec` alias + `create_prover_service` body.

use std::sync::Arc;

use async_trait::async_trait;
use ligate_stf::Runtime;
use sov_address::{EthereumAddress, FromVmAddress, MultiAddressEvm};
use sov_celestia_adapter::verifier::{CelestiaSpec, CelestiaVerifier, RollupParams};
use sov_celestia_adapter::CelestiaService;
use sov_db::ledger_db::LedgerDb;
use sov_db::storage_manager::NomtStorageManager;
use sov_mock_zkvm::{MockCodeCommitment, MockZkvm, MockZkvmCryptoSpec, MockZkvmHost};
use sov_modules_api::configurable_spec::ConfigurableSpec;
use sov_modules_api::execution_mode::Native;
use sov_modules_api::macros::config_value;
use sov_modules_api::{CryptoSpec, NodeEndpoints, Spec, ZkVerifier};
use sov_modules_rollup_blueprint::pluggable_traits::PluggableSpec;
use sov_modules_rollup_blueprint::proof_sender::SovApiProofSender;
use sov_modules_rollup_blueprint::{FullNodeBlueprint, RollupBlueprint, SequencerCreationReceipt};
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

type Hasher = <MockZkvmCryptoSpec as CryptoSpec>::Hasher;
type NativeStorage =
    NomtProverStorage<DefaultStorageSpec<Hasher>, <CelestiaSpec as DaSpec>::SlotHash>;

/// Concrete [`Spec`] used by [`CelestiaLigateRollup`].
///
/// Identical address shape to [`crate::MockRollupSpec`]
/// (`MultiAddressEvm` → 28-byte standard + 20-byte Ethereum). Only
/// the DA layer differs.
pub type CelestiaRollupSpec<M> = ConfigurableSpec<
    CelestiaSpec,
    MockZkvm,
    MockZkvm,
    MultiAddressEvm,
    M,
    MockZkvmCryptoSpec,
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
        // MockZkvm host on both legs. The Celestia DA verifier is
        // real — proofs are mock, but DA inclusion proofs are
        // canonically verified.
        let inner_vm = MockZkvmHost::new_non_blocking();
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
