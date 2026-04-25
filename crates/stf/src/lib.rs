//! Ligate Chain state transition function.
//!
//! This crate composes the Ligate Chain runtime: a [`runtime::Runtime`] struct
//! gathering the SDK-required modules ([`sov_bank`],
//! [`sov_accounts`], [`sov_sequencer_registry`]) and Ligate's own
//! [`attestation`] module. The runtime is generic over a context `C`
//! and a DA spec `Da`, so the same composition runs against any DA
//! adapter the rollup binary plugs in.
//!
//! Three pieces live here:
//!
//! - [`runtime`]: the [`runtime::Runtime`] struct, its derive-generated
//!   `Genesis` / `DispatchCall` / `MessageCodec` / `DefaultRuntime`
//!   impls, and the manual `sov_modules_stf_blueprint::Runtime` impl
//!   that ties it back to the blueprint.
//! - [`genesis_config`]: a [`genesis_config::GenesisPaths`] struct
//!   mapping a directory of JSON files to the per-module genesis
//!   configs the runtime expects, plus a small validator that catches
//!   obvious genesis misconfigurations (e.g. the sequencer's lock
//!   token is not the genesis `$LGT` token).
//! - `hooks_impl`: the four hook traits (`TxHooks`, `ApplyBlobHooks`,
//!   `SlotHooks`, `FinalizeHook`) wired through the runtime so the
//!   blueprint can invoke them at the right points in the slot loop.

#![deny(missing_docs)]

pub mod genesis_config;
mod hooks_impl;
pub mod runtime;

#[cfg(test)]
mod tests;
