//! Ligate Chain state transition function.
//!
//! This crate is the trait-wiring half of the runtime: it wraps
//! [`ligate_stf_declaration::Runtime`] in a newtype, then implements
//! [`sov_modules_api::Runtime`] / `HasCapabilities` / `HasKernel`
//! plus the genesis loader. The runtime composition itself (the
//! per-module struct + derives) lives in `ligate-stf-declaration`.
//!
//! See [`ligate_stf_declaration`] for the module-selection rationale.

#![deny(missing_docs)]

pub mod genesis_config;
pub mod runtime;
#[cfg(feature = "native")]
mod runtime_capabilities;

#[cfg(test)]
mod tests;

pub use runtime::{GenesisConfig, Runtime, RuntimeCall, RuntimeEvent};
