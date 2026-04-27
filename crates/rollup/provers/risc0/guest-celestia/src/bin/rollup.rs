//! Risc0 guest re-running the Ligate Chain STF.
//!
//! The host builds a `Receipt` containing this binary's image
//! hash plus the `journal` we commit to during execution. Verifies
//! externally without re-running.
//!
//! Mirrors the SDK demo's `guest-celestia/src/bin/rollup.rs`. The
//! only structural difference: our `Spec` uses `MultiAddressEvm`
//! (no Solana variant) and our runtime is `ligate_stf::Runtime`.

#![no_main]

use ligate_stf::Runtime;
use sov_address::MultiAddressEvm;
use sov_celestia_adapter::types::Namespace;
use sov_celestia_adapter::verifier::{CelestiaSpec, CelestiaVerifier, RollupParams};
use sov_mock_zkvm::MockZkvm;
use sov_modules_api::configurable_spec::ConfigurableSpec;
use sov_modules_api::execution_mode::Zk;
use sov_modules_api::macros::config_value;
use sov_modules_stf_blueprint::StfBlueprint;
use sov_risc0_adapter::guest::Risc0Guest;
use sov_risc0_adapter::Risc0;
use sov_rollup_interface::da::DaVerifier;
use sov_rollup_interface::stf::StateTransitionVerifier;
use sov_state::nomt::zk_storage::NomtVerifierStorage;
use sov_state::DefaultStorageSpec;

type NomtStorage = NomtVerifierStorage<DefaultStorageSpec<sha2::Sha256>>;

/// Celestia namespace where the rollup posts its tx batches.
/// Sourced from `BATCH_NAMESPACE` in `constants.toml` so a guest
/// and the host can never disagree silently on namespacing.
pub const ROLLUP_BATCH_NAMESPACE: Namespace = Namespace::const_v0(config_value!("BATCH_NAMESPACE"));

/// Celestia namespace where the rollup posts its zk proofs.
pub const ROLLUP_PROOF_NAMESPACE: Namespace = Namespace::const_v0(config_value!("PROOF_NAMESPACE"));

risc0_zkvm::guest::entry!(main);

pub fn main() {
    let guest = Risc0Guest::new();
    let storage = NomtStorage::new();

    // Same per-module composition as the host runs natively. Only
    // the spec differs: `Zk` execution mode here, `Native` on the
    // host. The schema-derived `CHAIN_HASH` is identical across
    // both — that's how transactions signed for the host verify
    // inside the guest.
    let stf: StfBlueprint<
        ConfigurableSpec<
            CelestiaSpec,
            Risc0,
            MockZkvm,
            MultiAddressEvm,
            Zk,
            sov_risc0_adapter::Risc0CryptoSpec,
            NomtStorage,
        >,
        Runtime<_>,
    > = StfBlueprint::new();

    let rollup_params = RollupParams {
        rollup_batch_namespace: ROLLUP_BATCH_NAMESPACE,
        rollup_proof_namespace: ROLLUP_PROOF_NAMESPACE,
    };
    let stf_verifier = StateTransitionVerifier::new(stf, CelestiaVerifier::new(rollup_params));

    stf_verifier
        .run_block(guest, storage)
        .expect("Prover must be honest");
}
