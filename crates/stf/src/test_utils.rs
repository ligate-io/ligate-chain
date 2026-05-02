//! Test-only trait impls for the production [`Runtime`].
//!
//! Gated behind the `test-utils` cargo feature. The impls here let
//! external integration tests (e.g. `crates/rollup/tests/e2e_smoke.rs`)
//! drive the production runtime through the SDK's
//! [`sov_test_utils::runtime::TestRunner`] harness. Without these
//! impls, `TestRunner` only accepts macro-generated runtimes from
//! `generate_optimistic_runtime!`, which is fine for module-level
//! tests but skips the production wiring.
//!
//! Mirrors the `examples/demo-rollup/stf/src/test_utils.rs` pattern in
//! the Sovereign SDK. The SDK explicitly recommends rollups
//! hand-roll [`MinimalGenesis`] for their production [`Runtime`] when
//! they want full-runtime integration tests.

use sov_address::{EthereumAddress, FromVmAddress};
use sov_modules_api::{Genesis, Spec};
use sov_sequencer_registry::SequencerRegistry;
use sov_test_utils::runtime::traits::MinimalGenesis;

use crate::Runtime;

impl<S: Spec> MinimalGenesis<S> for Runtime<S>
where
    S::Address: FromVmAddress<EthereumAddress>,
{
    fn sequencer_registry_config(
        config: &Self::Config,
    ) -> &<SequencerRegistry<S> as Genesis>::Config {
        &config.sequencer_registry
    }
}
