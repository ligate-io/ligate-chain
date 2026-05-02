//! End-to-end smoke tests for the rollup blueprint.
//!
//! Asserts the wiring between [`ligate_rollup::MockLigateRollup`],
//! [`ligate_stf::Runtime`], and the SDK's blueprint traits all works
//! against the production address type ([`MultiAddressEvm`]) — not
//! the `sov_test_utils::TestSpec` we use in the stf crate's
//! lower-level tests.
//!
//! What we deliberately *don't* test here: actually booting a node
//! against RocksDB / mock DA / the sequencer task graph. That needs
//! a full `RollupConfig` (storage dirs, ports, prover paths) and is
//! the territory of an integration harness in Phase A.2.3, where
//! we'll have a real `devnet/rollup.toml` to load.

use std::fs;

use attestation::AttestationConfig;
use ligate_rollup::{MockLigateRollup, MockRollupSpec};
use ligate_stf::genesis_config::GenesisPaths;
use ligate_stf::runtime::GenesisConfig;
use ligate_stf::Runtime;
use sov_bank::config_gas_token_id;
use sov_modules_api::execution_mode::Native;
use sov_modules_api::Amount;
use sov_modules_rollup_blueprint::RollupBlueprint;
use sov_test_utils::runtime::genesis::optimistic::{
    HighLevelOptimisticGenesisConfig, MinimalOptimisticGenesisConfig,
};
use tempfile::TempDir;

/// Compile-time guard that the blueprint resolves the runtime to
/// `ligate_stf::Runtime<MockRollupSpec<Native>>`. If this signature
/// stops type-checking, somebody has accidentally swapped the
/// runtime under the blueprint.
#[allow(dead_code)]
fn _assert_blueprint_runtime_is_ligate_stf(
    rt: <MockLigateRollup<Native> as RollupBlueprint<Native>>::Runtime,
) -> Runtime<MockRollupSpec<Native>> {
    rt
}

#[test]
fn mock_blueprint_default_is_send_sync_static() {
    fn assert_send_sync_static<T: Send + Sync + 'static>(_: T) {}
    assert_send_sync_static(MockLigateRollup::<Native>::default());
}

#[test]
fn runtime_genesis_config_round_trips_for_production_spec() {
    // Builds a genesis directory the same way the rollup binary
    // would (one JSON per module under one directory), then drives
    // the blueprint's runtime trait — the same path
    // `MockLigateRollup::create_genesis_config` takes — and verifies
    // it loads + validates without error.
    type S = MockRollupSpec<Native>;

    let dir = TempDir::new().unwrap();

    // Build the 9 SDK module configs via the test-utils helper, add
    // attestation by hand.
    let high_level = HighLevelOptimisticGenesisConfig::<S>::generate();
    let attester_addr = high_level.initial_attester.user_info.address();
    let minimal: MinimalOptimisticGenesisConfig<S> = high_level.into();
    let basic = minimal.config;

    let attestation = AttestationConfig::<S> {
        treasury: attester_addr,
        lgt_token_id: config_gas_token_id(),
        attestation_fee: Amount(0),
        schema_registration_fee: Amount(0),
        attestor_set_fee: Amount(0),
        initial_attestor_sets: vec![],
        initial_schemas: vec![],
        max_builder_bps: attestation::DEFAULT_MAX_BUILDER_BPS,
    };

    let genesis = GenesisConfig::<S> {
        bank: basic.bank,
        accounts: basic.accounts,
        sequencer_registry: basic.sequencer_registry,
        operator_incentives: basic.operator_incentives,
        attester_incentives: basic.attester_incentives,
        prover_incentives: basic.prover_incentives,
        uniqueness: (),
        chain_state: basic.chain_state,
        blob_storage: (),
        attestation,
    };

    // Dump each module to <tempdir>/<module>.json — same convention
    // `GenesisPaths::from_dir` expects.
    for (file, json) in [
        ("bank.json", serde_json::to_vec_pretty(&genesis.bank).unwrap()),
        ("accounts.json", serde_json::to_vec_pretty(&genesis.accounts).unwrap()),
        (
            "sequencer_registry.json",
            serde_json::to_vec_pretty(&genesis.sequencer_registry).unwrap(),
        ),
        (
            "operator_incentives.json",
            serde_json::to_vec_pretty(&genesis.operator_incentives).unwrap(),
        ),
        (
            "attester_incentives.json",
            serde_json::to_vec_pretty(&genesis.attester_incentives).unwrap(),
        ),
        ("prover_incentives.json", serde_json::to_vec_pretty(&genesis.prover_incentives).unwrap()),
        ("chain_state.json", serde_json::to_vec_pretty(&genesis.chain_state).unwrap()),
        ("attestation.json", serde_json::to_vec_pretty(&genesis.attestation).unwrap()),
    ] {
        fs::write(dir.path().join(file), json).unwrap();
    }

    let paths = GenesisPaths::from_dir(dir.path());

    // Drive through the runtime trait the blueprint will call.
    let loaded = <Runtime<S> as sov_modules_api::Runtime<S>>::genesis_config(&paths)
        .expect("blueprint genesis_config should accept the round-tripped configs");

    assert_eq!(loaded.attestation.lgt_token_id, config_gas_token_id());
}
