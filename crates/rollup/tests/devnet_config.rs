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

use ligate_rollup::MockRollupSpec;
use ligate_stf::genesis_config::GenesisPaths;
use ligate_stf::Runtime;
use sov_address::MultiAddressEvm;
use sov_mock_da::storable::StorableMockDaService;
use sov_modules_api::execution_mode::Native;
use sov_stf_runner::{from_toml_path, RollupConfig};

/// Path to the repo's checked-in `devnet/` directory, regardless of
/// where `cargo test` is invoked from.
fn devnet_dir() -> PathBuf {
    // `CARGO_MANIFEST_DIR` is `crates/rollup/`; the devnet lives two
    // levels up.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/rollup/ has a workspace-root grandparent")
        .join("devnet")
}

#[test]
fn rollup_toml_parses_against_mock_blueprint_types() {
    // Catches TOML keys renamed/removed in the SDK between pins, or
    // keys we typoed in `devnet/rollup.toml`.
    let path = devnet_dir().join("rollup.toml");
    let _config: RollupConfig<MultiAddressEvm, StorableMockDaService> = from_toml_path(&path)
        .unwrap_or_else(|e| {
            panic!("devnet/rollup.toml failed to parse: {e:?}");
        });
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
