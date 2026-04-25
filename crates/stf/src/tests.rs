//! Smoke tests for the runtime composition.
//!
//! These are deliberately compile-time-flavoured: they instantiate
//! [`Runtime`] against the SDK's `DefaultContext` and `MockDaSpec`
//! and exercise the derive-generated trait impls, so a regression in
//! generic bounds, derive macros, or the `sov_modules_stf_blueprint`
//! version pin shows up here rather than failing only when the rollup
//! binary is built.
//!
//! Genesis/runtime-loop behaviour is exercised by the rollup binary's
//! own tests (Phase A.2) where the real DA service, ledger DB, and
//! sequencer queue exist. Here we only verify wiring.

use sov_mock_da::MockDaSpec;
use sov_modules_api::default_context::DefaultContext;
use sov_modules_stf_blueprint::Runtime as RuntimeTrait;

use crate::runtime::Runtime;

/// `Runtime<DefaultContext, MockDaSpec>` is the same instantiation the
/// rollup binary will build against. If this stops compiling, every
/// downstream consumer breaks too.
#[allow(dead_code)]
type RuntimeUnderTest = Runtime<DefaultContext, MockDaSpec>;

/// `Runtime: Default` is required by the blueprint so it can build a
/// fresh instance for genesis. The derive should give us this for
/// free; this test exists to fail loudly if a future module added to
/// the runtime forgets `Default`.
#[test]
fn runtime_is_default() {
    let _: RuntimeUnderTest = Default::default();
}

/// Confirm the runtime impls `sov_modules_stf_blueprint::Runtime` and
/// that the associated `GenesisConfig` and `GenesisPaths` types
/// resolve. Catches derive-macro regressions where a module field
/// stops satisfying `Genesis`.
#[test]
fn runtime_implements_stf_blueprint_runtime() {
    fn assert_impl<C, Da, R>()
    where
        C: sov_modules_api::Context,
        Da: sov_rollup_interface::da::DaSpec,
        R: RuntimeTrait<C, Da>,
    {
    }
    assert_impl::<DefaultContext, MockDaSpec, RuntimeUnderTest>();
}
