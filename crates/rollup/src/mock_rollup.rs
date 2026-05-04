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
use sov_modules_api::{CryptoSpec, NodeEndpoints, Spec, ZkVerifier};
use sov_modules_rollup_blueprint::pluggable_traits::PluggableSpec;
use sov_modules_rollup_blueprint::proof_sender::SovApiProofSender;
use sov_modules_rollup_blueprint::{FullNodeBlueprint, RollupBlueprint, SequencerCreationReceipt};
use sov_rollup_full_node_interface::StateUpdateReceiver;
use sov_rollup_interface::da::DaSpec;
use sov_rollup_interface::node::SyncStatus;
use sov_sequencer::ProofBlobSender;
use sov_state::nomt::prover_storage::NomtProverStorage;
use sov_state::{DefaultStorageSpec, Storage};
use sov_stf_runner::processes::{ParallelProverService, ProverService, RollupProverConfig};
use sov_stf_runner::RollupConfig;

/// Marker type for the mock-DA / mock-zkVM rollup. Carries no state;
/// the [`Default`] / [`Clone`] / [`Copy`] impls let the blueprint
/// machinery construct it however it wants.
#[derive(Default, Clone, Copy, Debug)]
pub struct MockLigateRollup<M> {
    phantom: std::marker::PhantomData<M>,
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

    fn create_outer_code_commitment(
        &self,
    ) -> <<Self::ProverService as ProverService>::Verifier as ZkVerifier>::CodeCommitment {
        // MockZkvm accepts any commitment as valid; the default is
        // fine. A real prover swaps this for the production circuit
        // commitment in Phase A.4 (#xx).
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
        // Phase 2 of #110: spawn the block-height polling task here
        // because `LedgerDb` is only handed to us via this hook.
        // The task lives until the tokio runtime tears down.
        crate::metrics::spawn_block_height_task(
            ledger_db.clone(),
            crate::metrics::DEFAULT_BLOCK_HEIGHT_POLL_INTERVAL,
        );

        // Stock SDK helper. Wires the standard sequencer + ledger
        // RPCs and the runtime's per-module REST routers (currently
        // empty, see `ligate_stf::runtime_capabilities`).
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
        StorableMockDaService::from_config(rollup_config.da.clone(), shutdown_receiver).await
    }

    async fn create_prover_service(
        &self,
        _prover_config: RollupProverConfig,
        rollup_config: &RollupConfig<<Self::Spec as Spec>::Address, Self::DaService>,
        _da_service: &Self::DaService,
    ) -> Self::ProverService {
        // Even when proving is disabled at the operator level
        // (`SOV_PROVER_MODE` unset), the blueprint still needs a
        // `ProverService` instance. `MockZkvmHost::new_non_blocking`
        // produces one that accepts every proof — fine for a
        // mock-zkVM devnet.
        let inner_vm = MockZkvmHost::new_non_blocking();
        let outer_vm = MockZkvmHost::new_non_blocking();
        let da_verifier = Default::default();

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
