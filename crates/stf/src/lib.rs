//! Ligate Chain state transition function.
//!
//! Composes the runtime: a [`Runtime`] struct gathering the
//! SDK-required modules (bank, accounts, sequencer-registry,
//! chain-state, blob-storage, uniqueness, capabilities-related
//! kernel modules) plus economic incentives (attester, prover,
//! operator) and Ligate's own [`attestation`] module.
//!
//! # Module selection rationale
//!
//! The new SDK's reference `demo-stf` includes ~15 modules, several of
//! which we don't want active at v0:
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
//! Adding a module post-launch happens via a chain upgrade (#42). The
//! address shape (28-byte `MultiAddress` with EVM compat from
//! `sov-address/evm`) is the SDK's standard; it does NOT mean we
//! ship EVM execution. Address encoding is a separate concern from
//! VM availability.

#![deny(missing_docs)]

pub mod genesis_config;
pub mod runtime;

#[cfg(test)]
mod tests;

pub use runtime::{GenesisConfig, Runtime, RuntimeCall, RuntimeEvent};
