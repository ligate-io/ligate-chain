//! Ligate Chain runtime — declaration-only.
//!
//! Holds the [`Runtime`] composition struct and the types the
//! `Genesis` / `DispatchCall` / `Event` derive macros generate
//! alongside it. **No trait impls live here.** Trait wiring (the
//! SDK's `Runtime`, `HasCapabilities`, `HasKernel`, plus the
//! delegation forwarders) lives in `ligate-stf`, which wraps this
//! crate's [`Runtime`] in a newtype.
//!
//! The two-crate split exists so the build script in `ligate-stf`
//! can `use ligate_stf_declaration::Runtime` to feed
//! `sov_build::Options::apply_defaults`. Cargo forbids a build
//! script from depending on its own crate, so the runtime struct
//! has to live in a separate upstream crate. The SDK's
//! `demo-stf-declaration` / `demo-stf` split exists for the same
//! reason — we mirror it.
//!
//! # Module selection rationale
//!
//! The new SDK's reference `demo-stf` includes ~15 modules, several
//! of which we don't want active at v0:
//!
//! | Module | Status | Reason |
//! |---|---|---|
//! | `sov-bank` | included | mandatory; holds `$LGT` |
//! | `sov-accounts` | included | mandatory; per-address nonces |
//! | `sov-sequencer-registry` | included | sequencer permissioning + `$LGT` collateral |
//! | `sov-chain-state` | included | kernel; slot / block-header bookkeeping |
//! | `sov-blob-storage` | included | kernel; DA blob staging |
//! | `sov-uniqueness` | included | replay protection |
//! | `sov-attester-incentives` | included | kernel-economic, can't strip |
//! | `sov-prover-incentives` | included | kernel-economic, can't strip |
//! | `sov-operator-incentives` | included | kernel-economic, can't strip |
//! | `attestation` | included | the chain's product surface |
//! | `sov-evm` | omitted | EVM execution; v4 territory (#52) |
//! | `sov-hyperlane-integration` | omitted | cross-chain; v1+ if at all |
//! | `sov-paymaster` | omitted | sponsored gas; v0.5 with Iris (#55) |
//! | `sov-revenue-share` | omitted | overlaps with our schema-level `fee_routing_bps` |
//! | `sov-test-modules`, `sov-synthetic-load` | omitted | test/example only |
//!
//! Adding a module post-launch happens via a chain upgrade (#42).
//! The address shape (28-byte `MultiAddress` with EVM compat from
//! `sov-address/evm`) is the SDK's standard; it does NOT mean we
//! ship EVM execution. Address encoding is a separate concern from
//! VM availability.

#![deny(missing_docs)]
#![allow(unused_doc_comments)]

use sov_address::{EthereumAddress, FromVmAddress};
use sov_modules_api::macros::RuntimeRestApi;
use sov_modules_api::prelude::*;
use sov_modules_api::{DispatchCall, Event, Genesis, Hooks, MessageCodec, Spec};

/// Composed Ligate runtime.
///
/// Field order is significant for genesis: each module's
/// `Module::genesis` runs in declaration order. Modules that read
/// another module's state at genesis (e.g. `sequencer_registry`
/// reading `bank` to lock collateral, `attestation` reading `bank`
/// for the `$LGT` token id) must come after their dependencies.
///
/// The address bound `S::Address: FromVmAddress<EthereumAddress>`
/// comes from the SDK's standard module interfaces (bank/accounts
/// internally surface EVM-shaped addresses for the wallet/UI). It's
/// an encoding constraint, not an EVM-execution claim. We do not
/// include `sov-evm` in the Runtime; deploying Solidity contracts
/// is not a v0 feature. Solana / `Base58Address` support is only
/// needed when hyperlane modules are present, which we omit.
#[derive(Default, Clone, Genesis, Hooks, DispatchCall, Event, MessageCodec, RuntimeRestApi)]
pub struct Runtime<S: Spec>
where
    S::Address: FromVmAddress<EthereumAddress>,
{
    /// Bank module: holds `$LGT` and any other fungible tokens.
    /// Genesis-first because every module that charges fees or locks
    /// collateral reads bank state at genesis time.
    pub bank: sov_bank::Bank<S>,

    /// Accounts module: per-address nonces and pubkey bindings.
    /// Required by the blueprint for tx authentication.
    pub accounts: sov_accounts::Accounts<S>,

    /// Sequencer registry: gates DA-layer addresses allowed to
    /// submit blobs. Locks `$LGT` collateral from each registered
    /// sequencer.
    pub sequencer_registry: sov_sequencer_registry::SequencerRegistry<S>,

    /// Operator incentives: rewards for non-prover, non-attester
    /// operators (i.e. sequencers / fullnodes).
    pub operator_incentives: sov_operator_incentives::OperatorIncentives<S>,

    /// Attester incentives: bonded stake + slashing for attesters.
    /// Distinct from Ligate's `attestation` module, which is the
    /// product-level attestation registry; this one is the SDK's
    /// validity-attester economic layer for the optimistic kernel.
    pub attester_incentives: sov_attester_incentives::AttesterIncentives<S>,

    /// Prover incentives: bonded stake + slashing for ZK provers.
    pub prover_incentives: sov_prover_incentives::ProverIncentives<S>,

    /// Uniqueness: replay-protection bookkeeping (per-account nonces
    /// + a sliding window of recent tx hashes).
    pub uniqueness: sov_uniqueness::Uniqueness<S>,

    /// Chain state: slot / block-header bookkeeping. Provides the
    /// timestamp the attestation module will eventually read for
    /// `Attestation::timestamp` (currently stubbed to 0; lands in
    /// follow-up).
    pub chain_state: sov_chain_state::ChainState<S>,

    /// Blob storage: DA-blob staging area used by the kernel between
    /// blob arrival and STF execution.
    pub blob_storage: sov_blob_storage::BlobStorage<S>,

    /// Ligate attestation module: schemas, attestor sets,
    /// attestations, fee charging via `bank`. The chain's product
    /// surface.
    pub attestation: attestation::AttestationModule<S>,
}
