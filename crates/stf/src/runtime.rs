//! The Ligate Chain runtime.
//!
//! Mirrors the SDK's `demo-stf-declaration` shape (derive-driven
//! Runtime composition with the standard kernel-side modules) but
//! omits the modules listed in [`crate`]'s top-level docs.
//!
//! # Lifecycle
//!
//! 1. **Genesis.** The `Genesis` derive macro generates a per-module
//!    config aggregator (`GenesisConfig<S>`) and a `Runtime::genesis`
//!    method. Modules initialise in declaration order; we declare
//!    `bank` first so other modules that read its state at genesis
//!    (sequencer-registry, attestation) see populated balances.
//! 2. **Tx dispatch.** The `MessageCodec` + `DispatchCall` derives
//!    deserialise an incoming `RuntimeCall<S>` and route it to the
//!    matching module's `Module::call`. Hooks (declared via the
//!    `Hooks` derive plus our manual `hooks_impl`) run at the
//!    appropriate points in the slot loop.

#![allow(unused_doc_comments)]

use sov_address::{EthereumAddress, FromVmAddress};
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
/// include `sov-evm` in the Runtime; deploying Solidity contracts is
/// not a v0 feature. Solana / `Base58Address` support is only needed
/// when hyperlane modules are present, which we omit.
#[derive(Default, Clone, Genesis, Hooks, DispatchCall, Event, MessageCodec)]
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
