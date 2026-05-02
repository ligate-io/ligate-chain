//! Newtype wrapper around [`ligate_stf_declaration::Runtime`].
//!
//! The wrapper exists for one practical reason: the SDK's
//! [`sov_modules_api::Runtime`] trait, [`HasCapabilities`], and
//! [`HasKernel`] impls go on a type local to **this** crate. The
//! orphan rule blocks impl'ing them on
//! `ligate_stf_declaration::Runtime` directly.
//!
//! Field access stays ergonomic via [`Deref`](std::ops::Deref) /
//! [`DerefMut`](std::ops::DerefMut) — `runtime.bank` still works the
//! same way it would on the inner type. The forwarding `impl`
//! blocks in this file are pure delegation: they exist so the
//! wrapper satisfies the bounds the SDK super-trait `Runtime<S>`
//! requires (`DispatchCall`, `Genesis`, `BlockHooks`, `TxHooks`,
//! `FinalizeHook`, `RuntimeEventProcessor`, `HasRestApi`,
//! `EncodeCall<sov_bank::Bank<S>>`).
//!
//! [`HasCapabilities`]: sov_modules_api::capabilities::HasCapabilities
//! [`HasKernel`]: sov_modules_api::capabilities::HasKernel

use ligate_stf_declaration::Runtime as RuntimeInner;
use sov_address::{EthereumAddress, FromVmAddress};
use sov_modules_api::prelude::*;
use sov_modules_api::{
    AuthenticatedTransactionData, BlockHooks, Context, DispatchCall, EncodeCall, Genesis,
    GenesisState, ModuleError, ModuleId, ModuleInfo, NestedEnumUtils, RuntimeEventProcessor, Spec,
    StateCheckpoint, StateProvider, Storage, TxHooks, TxState, TypeErasedEvent, WorkingSet,
};
use sov_rollup_interface::da::DaSpec;

pub use ligate_stf_declaration::{GenesisConfig, RuntimeCall, RuntimeEvent};

/// The Ligate Chain runtime.
///
/// Newtype wrapping [`ligate_stf_declaration::Runtime`] so this crate
/// can host the trait impls that depend on local types (the
/// `runtime_capabilities` module). Use it exactly like the inner
/// runtime — `Deref` makes per-module field access transparent.
#[derive(Default, Clone)]
pub struct Runtime<S: Spec>(pub(crate) RuntimeInner<S>)
where
    S::Address: FromVmAddress<EthereumAddress>;

impl<S: Spec> std::ops::Deref for Runtime<S>
where
    S::Address: FromVmAddress<EthereumAddress>,
{
    type Target = RuntimeInner<S>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<S: Spec> std::ops::DerefMut for Runtime<S>
where
    S::Address: FromVmAddress<EthereumAddress>,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

// ---------------------------------------------------------------------------
// Trait forwarding.
//
// All of these are pure delegation to the inner runtime's
// derive-generated impls. Each block exists so the bound
// `sov_modules_api::Runtime<S>` (in `runtime_capabilities`)
// resolves against the wrapper. New impls only land here when the
// SDK super-trait grows a new bound.
// ---------------------------------------------------------------------------

impl<S: Spec> Genesis for Runtime<S>
where
    S::Address: FromVmAddress<EthereumAddress>,
{
    type Spec = S;
    type Config = GenesisConfig<S>;

    fn genesis(
        &mut self,
        genesis_rollup_header: &<<Self::Spec as Spec>::Da as DaSpec>::BlockHeader,
        config: &Self::Config,
        state: &mut impl GenesisState<Self::Spec>,
    ) -> Result<(), ModuleError> {
        self.0.genesis(genesis_rollup_header, config, state)
    }
}

impl<S: Spec> DispatchCall for Runtime<S>
where
    S::Address: FromVmAddress<EthereumAddress>,
{
    type Spec = S;
    type Decodable = RuntimeCall<S>;

    fn encode(decodable: &Self::Decodable) -> Vec<u8> {
        RuntimeInner::<S>::encode(decodable)
    }

    fn dispatch_call<I: StateProvider<Self::Spec>>(
        &mut self,
        message: Self::Decodable,
        state: &mut WorkingSet<Self::Spec, I>,
        context: &Context<Self::Spec>,
    ) -> Result<(), ModuleError> {
        self.0.dispatch_call(message, state, context)
    }

    fn module_id(&self, message: &Self::Decodable) -> &ModuleId {
        self.0.module_id(message)
    }

    fn module_info(
        &self,
        discriminant: <Self::Decodable as NestedEnumUtils>::Discriminants,
    ) -> &dyn ModuleInfo<Spec = Self::Spec> {
        self.0.module_info(discriminant)
    }
}

impl<S: Spec> EncodeCall<sov_bank::Bank<S>> for Runtime<S>
where
    S::Address: FromVmAddress<EthereumAddress>,
{
    fn encode_call(data: <sov_bank::Bank<S> as sov_modules_api::Module>::CallMessage) -> Vec<u8> {
        <RuntimeInner<S> as EncodeCall<sov_bank::Bank<S>>>::encode_call(data)
    }

    fn to_decodable(
        data: <sov_bank::Bank<S> as sov_modules_api::Module>::CallMessage,
    ) -> Self::Decodable {
        <RuntimeInner<S> as EncodeCall<sov_bank::Bank<S>>>::to_decodable(data)
    }
}

impl<S: Spec> EncodeCall<attestation::AttestationModule<S>> for Runtime<S>
where
    S::Address: FromVmAddress<EthereumAddress>,
{
    fn encode_call(
        data: <attestation::AttestationModule<S> as sov_modules_api::Module>::CallMessage,
    ) -> Vec<u8> {
        <RuntimeInner<S> as EncodeCall<attestation::AttestationModule<S>>>::encode_call(data)
    }

    fn to_decodable(
        data: <attestation::AttestationModule<S> as sov_modules_api::Module>::CallMessage,
    ) -> Self::Decodable {
        <RuntimeInner<S> as EncodeCall<attestation::AttestationModule<S>>>::to_decodable(data)
    }
}

impl<S: Spec> BlockHooks for Runtime<S>
where
    S::Address: FromVmAddress<EthereumAddress>,
{
    type Spec = S;

    fn begin_rollup_block_hook(
        &mut self,
        visible_hash: &<<Self::Spec as Spec>::Storage as Storage>::Root,
        state: &mut StateCheckpoint<Self::Spec>,
    ) {
        self.0.begin_rollup_block_hook(visible_hash, state);
    }

    fn end_rollup_block_hook(&mut self, state: &mut StateCheckpoint<Self::Spec>) {
        self.0.end_rollup_block_hook(state);
    }
}

impl<S: Spec> TxHooks for Runtime<S>
where
    S::Address: FromVmAddress<EthereumAddress>,
{
    type Spec = S;

    fn pre_dispatch_tx_hook<T: TxState<Self::Spec>>(
        &mut self,
        tx: &AuthenticatedTransactionData<Self::Spec>,
        state: &mut T,
    ) -> anyhow::Result<()> {
        self.0.pre_dispatch_tx_hook(tx, state)
    }

    fn post_dispatch_tx_hook<T: TxState<Self::Spec>>(
        &mut self,
        tx: &AuthenticatedTransactionData<Self::Spec>,
        ctx: &Context<Self::Spec>,
        state: &mut T,
    ) -> anyhow::Result<()> {
        self.0.post_dispatch_tx_hook(tx, ctx, state)
    }
}

#[cfg(feature = "native")]
impl<S: Spec> sov_modules_api::FinalizeHook for Runtime<S>
where
    S::Address: FromVmAddress<EthereumAddress>,
{
    type Spec = S;

    fn finalize_hook(
        &mut self,
        root_hash: &<<Self::Spec as Spec>::Storage as Storage>::Root,
        state: &mut impl sov_modules_api::AccessoryStateReaderAndWriter,
    ) {
        self.0.finalize_hook(root_hash, state);
    }
}

impl<S: Spec> RuntimeEventProcessor for Runtime<S>
where
    S::Address: FromVmAddress<EthereumAddress>,
{
    type RuntimeEvent = RuntimeEvent<S>;

    fn convert_to_runtime_event(event: TypeErasedEvent) -> Option<Self::RuntimeEvent> {
        RuntimeInner::<S>::convert_to_runtime_event(event)
    }
}

#[cfg(feature = "native")]
impl<S: Spec> sov_modules_api::rest::HasRestApi<S> for Runtime<S>
where
    S::Address: FromVmAddress<EthereumAddress>,
{
    fn rest_api(
        &self,
        state: sov_modules_api::rest::ApiState<S>,
    ) -> sov_modules_api::prelude::axum::Router<()> {
        self.0.rest_api(state)
    }

    fn openapi_spec(&self) -> Option<sov_modules_api::prelude::utoipa::openapi::OpenApi> {
        self.0.openapi_spec()
    }
}
