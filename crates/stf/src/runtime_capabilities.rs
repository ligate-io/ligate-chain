//! `Runtime` trait wiring for the rollup binary.
//!
//! [`crate::runtime::Runtime`] is the runtime _composition_ — fields,
//! derive macros, doc comments. This module implements the three
//! traits the SDK's STF blueprint needs to actually drive the
//! runtime against blocks:
//!
//! - [`sov_modules_api::Runtime`]: top-level wiring — `CHAIN_HASH`,
//!   `Auth`, `GenesisConfig`, `endpoints`. Glues the per-module
//!   derives together into a node-runnable surface.
//! - [`HasCapabilities`]: surfaces the bundle of modules the
//!   STF/sequencer use during slot processing (bank for fees,
//!   sequencer-registry for blob auth, accounts for nonces, etc.).
//!   We use the SDK's [`StandardProvenRollupCapabilities`] with no
//!   gas payer — `sov-paymaster` is omitted from the runtime.
//! - [`HasKernel`]: chooses the slot-processing kernel. We pick
//!   [`SoftConfirmationsKernel`] (process all blobs in DA order;
//!   match the SDK demo's default).
//!
//! ## `CHAIN_HASH` is currently a placeholder
//!
//! The canonical value is generated at build time from the runtime's
//! universal-wallet schema (see `sov_build::Options::apply_defaults`).
//! Wiring that build script is its own slice — it requires every
//! `Decodable` payload to derive `JsonSchema`, which our
//! `attestation::CallMessage` does not yet. Until that lands, the
//! runtime ships with `CHAIN_HASH = [0u8; 32]`.
//!
//! Practical effect: standard transactions will authenticate against
//! a chain id of all-zeros. That's fine for an internal devnet smoke
//! test (client and node use the same constant), but the production
//! devnet must replace this before opening to external sequencers —
//! otherwise a rollup with a different runtime composition could
//! accept replays of our transactions. Tracked alongside the build
//! script slice.

use sov_address::{EthereumAddress, FromVmAddress};
use sov_capabilities::StandardProvenRollupCapabilities;
use sov_kernels::soft_confirmations::SoftConfirmationsKernel;
use sov_modules_api::capabilities::{
    Guard, HasCapabilities, HasKernel, KernelWithSlotMapping, RollupAuthenticator,
};
use sov_modules_api::rest::{ApiState, HasRestApi};
use sov_modules_api::{NodeEndpoints, OperatingMode, Spec};
use std::sync::Arc;

use crate::genesis_config::{create_genesis_config, GenesisPaths};
use crate::runtime::{GenesisConfig, Runtime};

/// Placeholder chain hash. See module-level docs for why this is
/// not yet derived from the universal-wallet schema, and why the
/// production devnet must replace it before opening to external
/// sequencers.
pub const PLACEHOLDER_CHAIN_HASH: [u8; 32] = [0u8; 32];

impl<S: Spec> sov_modules_api::Runtime<S> for Runtime<S>
where
    S::Address: FromVmAddress<EthereumAddress>,
{
    const CHAIN_HASH: [u8; 32] = PLACEHOLDER_CHAIN_HASH;

    type GenesisConfig = GenesisConfig<S>;
    type GenesisInput = GenesisPaths;
    type ModuleExecutionConfig = ();
    type Auth = RollupAuthenticator<S, Self>;

    fn endpoints(api_state: ApiState<S>) -> NodeEndpoints {
        // Per-module REST routers (e.g. `GET /modules/bank/tokens`),
        // plumbed through `HasRestApi` from the `RuntimeRestApi`
        // derive on `Runtime`. The SDK demo additionally wires the
        // universal-wallet schema and dedup endpoints, both gated
        // on the build-script-generated `SCHEMA_JSON` we don't ship
        // yet (see module-level docs on `CHAIN_HASH`); they land in
        // the same slice as the build script.
        NodeEndpoints { axum_router: Self::default().rest_api(api_state), ..Default::default() }
    }

    fn genesis_config(input: &Self::GenesisInput) -> anyhow::Result<Self::GenesisConfig> {
        // Promote the typed `GenesisError` (used by ligate-stf
        // internals) to `anyhow::Error` for the SDK's blueprint API,
        // which is `anyhow`-flavoured at this boundary.
        Ok(create_genesis_config(input)?)
    }

    fn operating_mode(genesis: &Self::GenesisConfig) -> OperatingMode {
        genesis.chain_state.operating_mode
    }

    fn wrap_call(
        auth_data: <Self::Auth as sov_modules_api::capabilities::TransactionAuthenticator<S>>::Decodable,
    ) -> <Self as sov_modules_api::DispatchCall>::Decodable {
        // `RollupAuthenticator` has a single decoded variant — it
        // dispatches directly to `Rt::Decodable`. So `wrap_call` is
        // the identity. (Compare the SDK demo's matching `Evm /
        // Solana / Standard` arms — we have none of those.)
        auth_data
    }
}

impl<S: Spec> HasCapabilities<S> for Runtime<S>
where
    S::Address: FromVmAddress<EthereumAddress>,
{
    /// We omit `sov-paymaster`, so the gas-payer slot is `()`. The
    /// SDK's `StandardProvenRollupCapabilities` defaults `GasPayer`
    /// to `()` precisely for this case.
    type Capabilities<'a> = StandardProvenRollupCapabilities<'a, S>;
    type SequencingData = sov_modules_api::HDTimestamp;

    fn capabilities(&mut self) -> Guard<Self::Capabilities<'_>> {
        Guard::new(StandardProvenRollupCapabilities {
            bank: &mut self.bank,
            gas_payer: (),
            sequencer_registry: &mut self.sequencer_registry,
            accounts: &mut self.accounts,
            uniqueness: &mut self.uniqueness,
            chain_state: &mut self.chain_state,
            operator_incentives: &mut self.operator_incentives,
            prover_incentives: &mut self.prover_incentives,
            attester_incentives: &mut self.attester_incentives,
        })
    }
}

impl<S: Spec> HasKernel<S> for Runtime<S>
where
    S::Address: FromVmAddress<EthereumAddress>,
{
    type Kernel<'a> = SoftConfirmationsKernel<'a, S>;

    fn inner(&mut self) -> Guard<Self::Kernel<'_>> {
        Guard::new(SoftConfirmationsKernel {
            chain_state: &mut self.chain_state,
            blob_storage: &mut self.blob_storage,
        })
    }

    fn kernel_with_slot_mapping(&self) -> Arc<dyn KernelWithSlotMapping<S>> {
        Arc::new(self.chain_state.clone())
    }
}
