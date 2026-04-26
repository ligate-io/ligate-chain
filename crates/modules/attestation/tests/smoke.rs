//! Smoke tests that prove the attestation module composes against
//! the new SDK's `Module` trait and runtime macros.
//!
//! These exist as the day-4 checkpoint of the SDK upgrade work
//! (#59): minimal coverage that wakes up if the module's generic
//! bounds, derive macros, or `Module::Spec` shape regress. Real
//! protocol-level test coverage lives in the per-handler test
//! modules that get rebuilt on the new test infrastructure during
//! days 5-6 of #59.

use attestation::AttestationModule;
use sov_test_utils::generate_optimistic_runtime;

// Compose a minimal optimistic runtime that contains only the
// attestation module. The macro generates the `Runtime` struct,
// `GenesisConfig` aggregator, dispatch enum, and event enum the
// test runner needs to exercise the module.
generate_optimistic_runtime!(AttestationRuntime <= attestation: AttestationModule<S>);

type S = sov_test_utils::TestSpec;
type RT = AttestationRuntime<S>;

/// `Runtime: Default` is required by the SDK's `TestRunner`
/// scaffolding so the framework can build a fresh runtime per
/// genesis. Catches macro-derive regressions where a future module
/// addition forgets `Default`.
#[test]
fn runtime_is_default() {
    let _runtime: RT = Default::default();
}

/// `AttestationModule<S>: Default` is required by the SDK's RPC
/// trait derives (`<Module>::default().method(...)`). Independent
/// of the runtime composition; isolates the module-level derive.
#[test]
fn module_is_default() {
    let _module: AttestationModule<S> = Default::default();
}
