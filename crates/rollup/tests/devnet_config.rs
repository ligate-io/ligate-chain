//! Validate the checked-in `devnet/` config end-to-end.
//!
//! These tests guard against drift between the runtime types and the
//! committed JSON / TOML. If a module's `Config` shape changes, or a
//! cross-module invariant tightens, the failure surfaces here before
//! anyone runs the binary.
//!
//! Tests are gated to the same `MockLigateRollup` blueprint shape the
//! binary uses, so the address type (`MultiAddressEvm`) and DA
//! service (`StorableMockDaService`) are exactly what production
//! code paths see.

use std::path::PathBuf;

use ligate_rollup::chain_config::load_split_config;
use ligate_rollup::{CelestiaRollupSpec, MockRollupSpec};
use ligate_stf::genesis_config::GenesisPaths;
use ligate_stf::Runtime;
use sov_address::MultiAddressEvm;
use sov_celestia_adapter::CelestiaService;
use sov_mock_da::storable::StorableMockDaService;
use sov_modules_api::execution_mode::Native;
use sov_stf_runner::RollupConfig;

/// Path to a checked-in `devnet*/` directory, regardless of where
/// `cargo test` is invoked from.
fn config_dir(name: &str) -> PathBuf {
    // `CARGO_MANIFEST_DIR` is `crates/rollup/`; the configs live two
    // levels up.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/rollup/ has a workspace-root grandparent")
        .join(name)
}

fn devnet_dir() -> PathBuf {
    config_dir("devnet")
}

fn devnet_1_dir() -> PathBuf {
    config_dir("devnet-1")
}

#[test]
fn rollup_toml_parses_against_mock_blueprint_types() {
    // Catches TOML keys renamed/removed in the SDK between pins, or
    // keys we typoed in `devnet/rollup.toml`. Uses the same two-pass
    // split (`[chain]` extracted, residual handed to the SDK) the
    // binary uses; see #181.
    let path = devnet_dir().join("rollup.toml");
    let (chain, residual) = load_split_config(&path)
        .unwrap_or_else(|e| panic!("devnet/rollup.toml failed [chain] split: {e:?}"));
    let _config: RollupConfig<MultiAddressEvm, StorableMockDaService> = toml::from_str(&residual)
        .unwrap_or_else(|e| panic!("devnet/rollup.toml residual failed to parse: {e:?}"));
    // Pin the committed chain id so a typo in the TOML (e.g.
    // accidentally bumping localnet to devnet on the localnet config)
    // surfaces here rather than at first boot.
    assert_eq!(chain.chain_id, "ligate-localnet");
}

#[test]
fn celestia_toml_parses_against_celestia_blueprint_types() {
    // Same drift guard, but for the Celestia DA flavour. The
    // `[da]` section's TOML schema differs entirely between
    // mock-DA and Celestia (different fields, different validation),
    // so the two configs need independent parse coverage. The
    // checked-in `signer_private_key` is a placeholder; operators
    // override via `SOV_CELESTIA_SIGNER_KEY` at runtime.
    let path = devnet_dir().join("celestia.toml");
    let (chain, residual) = load_split_config(&path)
        .unwrap_or_else(|e| panic!("devnet/celestia.toml failed [chain] split: {e:?}"));
    let _config: RollupConfig<MultiAddressEvm, CelestiaService> = toml::from_str(&residual)
        .unwrap_or_else(|e| panic!("devnet/celestia.toml residual failed to parse: {e:?}"));
    assert_eq!(chain.chain_id, "ligate-localnet");
}

#[test]
fn genesis_jsons_load_and_pass_cross_module_validation() {
    // End-to-end through the same path the binary takes:
    //   GenesisPaths -> create_genesis_config -> validate_config
    // Catches schema mismatches in any per-module JSON, plus the
    // `attestation.lgt_token_id == config_gas_token_id()` invariant.
    type S = MockRollupSpec<Native>;
    let paths = GenesisPaths::from_dir(devnet_dir().join("genesis"));
    let _config = <Runtime<S> as sov_modules_api::Runtime<S>>::genesis_config(&paths)
        .unwrap_or_else(|e| {
            panic!("devnet/genesis/ failed to load + validate: {e:?}");
        });
}

// ============================================================================
// devnet-1/ public-devnet artifacts (#188)
//
// Same drift coverage as the localnet tests above, applied to the
// public-devnet rung of the chain-id ladder. A typo on `chain_id` or
// a TOML key drift in the public configs would otherwise only surface
// at deploy time; these tests catch it at PR time.
// ============================================================================

#[test]
fn devnet_1_celestia_toml_parses_against_celestia_blueprint_types() {
    let path = devnet_1_dir().join("celestia.toml");
    let (chain, residual) = load_split_config(&path)
        .unwrap_or_else(|e| panic!("devnet-1/celestia.toml failed [chain] split: {e:?}"));
    let _config: RollupConfig<MultiAddressEvm, CelestiaService> = toml::from_str(&residual)
        .unwrap_or_else(|e| panic!("devnet-1/celestia.toml residual failed to parse: {e:?}"));
    // Pin the committed chain id so a typo (e.g. bumping to
    // `ligate-1` on the devnet-1 config) breaks here rather than at
    // first boot.
    assert_eq!(chain.chain_id, "ligate-devnet-1");
}

#[test]
fn devnet_1_genesis_jsons_load_and_pass_cross_module_validation() {
    // The committed devnet-1/genesis/ uses placeholder lig1 addresses
    // shared with devnet/genesis/, plus a Celestia bech32 placeholder
    // for `seq_da_address` (test fixtures, not for production deploy;
    // operators substitute via the genesis-tool — see #191 / #325).
    // devnet-1 is Celestia-only by design, so validate against the
    // Celestia rollup spec (which parses `seq_da_address` as bech32).
    type S = CelestiaRollupSpec<Native>;
    let paths = GenesisPaths::from_dir(devnet_1_dir().join("genesis"));
    let _config = <Runtime<S> as sov_modules_api::Runtime<S>>::genesis_config(&paths)
        .unwrap_or_else(|e| {
            panic!("devnet-1/genesis/ failed to load + validate: {e:?}");
        });
}

#[test]
fn devnet_1_does_not_ship_a_rollup_toml() {
    // Devnet-1 is Celestia-only by design (`devnet-1/README.md`):
    // MockDA only exists in `devnet/` for local smoke testing. If
    // someone copy-pastes `devnet/rollup.toml` into `devnet-1/` by
    // mistake (a plausible cargo-culting accident), operators booting
    // with `--rollup-config-path devnet-1/rollup.toml` would silently
    // get a MockDA flavour instead of Celestia and never publish to
    // Mocha. Catch the file-existence drift here.
    //
    // Closes #190 (the public-devnet config drift coverage; the
    // celestia.toml + genesis tests above pin the chain_id and module
    // shapes; this one pins the file inventory).
    let stray = devnet_1_dir().join("rollup.toml");
    assert!(
        !stray.exists(),
        "devnet-1/ is Celestia-only by design — rollup.toml should not exist; \
         see devnet-1/README.md. Found: {}",
        stray.display(),
    );
}
