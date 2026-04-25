//! The Ligate Chain runtime.
//!
//! On a high level, the rollup node receives serialised call messages
//! from the DA layer and dispatches them as atomic transactions. The
//! `Runtime` owns the module composition, derives the `Genesis`,
//! `DispatchCall`, and `MessageCodec` glue, and plugs into the
//! `sov_modules_stf_blueprint::Runtime` trait so the blueprint can
//! drive genesis and tx dispatch at slot boundaries.
//!
//! Lifecycle:
//!
//! 1. Genesis. The blueprint calls `Runtime::genesis(config)`
//!    (derive-generated), which iterates over the module fields in the
//!    order they're declared on the struct and invokes each module's
//!    own `Module::genesis`. Order matters: modules referenced by
//!    later modules must initialise first. We declare `bank`,
//!    `accounts`, `sequencer_registry`, `attestation`, in that
//!    order, since `sequencer_registry` reads `bank` state to lock
//!    sequencer collateral, and `attestation` reads `bank` state to
//!    charge fees.
//!
//! 2. Tx dispatch. The blueprint deserialises a `CallMessage` against
//!    the runtime's `MessageCodec` derive, then invokes the matching
//!    module's `Module::call`. Hooks (declared in `hooks_impl.rs`)
//!    run before and after each dispatch.

#![allow(unused_doc_comments)]

use sov_modules_api::macros::DefaultRuntime;
use sov_modules_api::{Context, DispatchCall, Genesis, MessageCodec, Spec};
use sov_rollup_interface::da::DaSpec;

use crate::genesis_config::GenesisPaths;

/// Composed Ligate runtime.
///
/// Field order is significant for genesis: each module is initialised
/// in declaration order, and a module that depends on another's state
/// (e.g. attestation reading the `$LGT` token from bank) must come
/// after its dependencies.
#[derive(Genesis, DispatchCall, MessageCodec, DefaultRuntime)]
#[serialization(borsh::BorshDeserialize, borsh::BorshSerialize)]
#[serialization(serde::Serialize, serde::Deserialize)]
pub struct Runtime<C: Context, Da: DaSpec> {
    /// Bank module: holds `$LGT` and any other fungible tokens. Must
    /// genesis first because `sequencer_registry` and `attestation`
    /// both read its state at genesis.
    pub bank: sov_bank::Bank<C>,

    /// Accounts module: per-address nonces and pubkey bindings.
    /// Required by the blueprint for tx authentication.
    pub accounts: sov_accounts::Accounts<C>,

    /// Sequencer registry: gates which DA-layer addresses are allowed
    /// to submit blobs as a sequencer. Locks `$LGT` collateral from
    /// each registered sequencer at genesis or via `register` calls.
    pub sequencer_registry: sov_sequencer_registry::SequencerRegistry<C, Da>,

    /// Ligate attestation module: schemas, attestor sets, attestations,
    /// fee charging via `bank`. The chain's actual product surface.
    pub attestation: attestation::AttestationModule<C>,
}

impl<C, Da> sov_modules_stf_blueprint::Runtime<C, Da> for Runtime<C, Da>
where
    C: Context,
    Da: DaSpec,
{
    type GenesisConfig = GenesisConfig<C, Da>;

    type GenesisPaths = GenesisPaths;

    fn rpc_methods(storage: <C as Spec>::Storage) -> jsonrpsee::RpcModule<()> {
        get_rpc_methods::<C, Da>(storage)
    }

    fn genesis_config(
        genesis_paths: &Self::GenesisPaths,
    ) -> Result<Self::GenesisConfig, anyhow::Error> {
        crate::genesis_config::get_genesis_config(genesis_paths)
    }
}

/// Build the runtime's RPC module surface.
///
/// Hand-rolled rather than `#[expose_rpc]`-derived because the
/// attestation module has no RPC yet (#21). The hand-rolled version
/// mirrors the macro output exactly: an [`RpcStorage`] helper that
/// holds storage and impls each SDK module's `RpcImpl` trait via
/// `get_working_set`, then merges every module's `RpcServer` into a
/// single [`jsonrpsee::RpcModule`].
///
/// Once #21 lands and adds `#[rpc_gen]` to attestation, replace this
/// function plus [`RpcStorage`] with a `#[expose_rpc]` annotation on
/// the [`Runtime`] struct above. The blueprint's `register_rpc`
/// helper layers ledger and sequencer RPC on top of this in the
/// rollup binary; that's not our concern here.
pub fn get_rpc_methods<C: Context, Da: DaSpec>(
    storage: <C as Spec>::Storage,
) -> jsonrpsee::RpcModule<()> {
    use sov_accounts::AccountsRpcServer;
    use sov_bank::BankRpcServer;
    use sov_sequencer_registry::SequencerRegistryRpcServer;

    let mut module = jsonrpsee::RpcModule::new(());
    let r = RpcStorage::<C, Da> { storage, _phantom: core::marker::PhantomData };

    module.merge(BankRpcServer::<C>::into_rpc(r.clone())).expect("merging bank rpc cannot fail");
    module
        .merge(AccountsRpcServer::<C>::into_rpc(r.clone()))
        .expect("merging accounts rpc cannot fail");
    module
        .merge(SequencerRegistryRpcServer::<C, Da>::into_rpc(r.clone()))
        .expect("merging sequencer registry rpc cannot fail");

    module
}

/// Per-runtime storage handle that implements each SDK module's
/// `RpcImpl` trait. Cheap to clone (clones the underlying storage
/// handle, not its contents). Constructed once per
/// [`get_rpc_methods`] invocation and handed off to each
/// `<Module>RpcServer::into_rpc` call.
pub struct RpcStorage<C: Context, Da: DaSpec> {
    storage: <C as Spec>::Storage,
    _phantom: core::marker::PhantomData<fn() -> Runtime<C, Da>>,
}

impl<C: Context, Da: DaSpec> Clone for RpcStorage<C, Da> {
    fn clone(&self) -> Self {
        Self { storage: self.storage.clone(), _phantom: core::marker::PhantomData }
    }
}

impl<C: Context, Da: DaSpec> sov_bank::BankRpcImpl<C> for RpcStorage<C, Da> {
    fn get_working_set(&self) -> sov_modules_api::WorkingSet<C> {
        sov_modules_api::WorkingSet::new(self.storage.clone())
    }
}

impl<C: Context, Da: DaSpec> sov_accounts::AccountsRpcImpl<C> for RpcStorage<C, Da> {
    fn get_working_set(&self) -> sov_modules_api::WorkingSet<C> {
        sov_modules_api::WorkingSet::new(self.storage.clone())
    }
}

impl<C: Context, Da: DaSpec> sov_sequencer_registry::SequencerRegistryRpcImpl<C, Da>
    for RpcStorage<C, Da>
{
    fn get_working_set(&self) -> sov_modules_api::WorkingSet<C> {
        sov_modules_api::WorkingSet::new(self.storage.clone())
    }
}
