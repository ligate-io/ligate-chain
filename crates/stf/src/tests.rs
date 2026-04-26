//! Compile-only smoke tests for the runtime composition.
//!
//! The real end-to-end tests (genesis-from-JSON, full slot execution,
//! cross-module invariants) live behind the `native` feature once
//! `genesis_config::create_genesis_config` lands in the next slice.
//! Here we just want to know that the [`Runtime`] type instantiates
//! against a concrete [`Spec`] and that all the derive-generated items
//! (`GenesisConfig`, `RuntimeCall`, `RuntimeEvent`) are reachable from
//! the public API.

use sov_address::MultiAddressEvm;
use sov_modules_api::configurable_spec::ConfigurableSpec;
use sov_modules_api::execution_mode::Native;
use sov_test_utils::{MockDaSpec, MockZkvm};

use crate::{Runtime, RuntimeCall, RuntimeEvent};

/// Concrete spec used for the smoke tests.
///
/// We can't reuse [`sov_test_utils::TestSpec`] because its `Address`
/// is the plain 28-byte [`sov_modules_api::Address`], which doesn't
/// implement `FromVmAddress<EthereumAddress>`. The runtime requires
/// that bound (see [`Runtime`]'s where clause), so we slot
/// [`MultiAddressEvm`] in via [`ConfigurableSpec`] instead.
type TestSpec = ConfigurableSpec<MockDaSpec, MockZkvm, MockZkvm, MultiAddressEvm, Native>;

#[test]
fn runtime_default_instantiates() {
    // If this compiles and runs, the derive macros expanded without
    // a missing-trait error and every module's `Default` impl is
    // reachable. That's enough for a scaffolding PR — the real
    // genesis tests come after `create_genesis_config` lands.
    let _runtime: Runtime<TestSpec> = Runtime::default();
}

#[test]
fn derive_generated_types_are_reachable() {
    // Ensures the `DispatchCall` / `Event` derives on `Runtime`
    // produced the expected sibling types, and that they're
    // re-exported from the crate root. The function bodies are
    // inert — we only care that the types name-resolve and satisfy
    // their bounds against `TestSpec`.
    fn _typecheck(_call: Option<RuntimeCall<TestSpec>>, _event: Option<RuntimeEvent<TestSpec>>) {}
    _typecheck(None, None);
}
