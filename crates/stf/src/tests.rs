//! Compile-only smoke tests + genesis-loader unit tests.
//!
//! The compile-only tests verify the runtime's derive macros expanded
//! cleanly. The loader tests cover [`create_genesis_config`] and
//! [`validate_config`] against a programmatically-assembled happy
//! genesis (round-trip through JSON + each cross-module failure mode).
//!
//! All `genesis_loader_*` tests use the SDK's
//! [`sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig`]
//! to build the 9 SDK-managed module configs in one shot. We add the
//! 10th — our [`attestation::AttestationConfig`] — by hand, since
//! that's the module whose invariants this loader exists to enforce.

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

#[cfg(feature = "native")]
#[test]
fn chain_hash_is_schema_derived_not_placeholder() {
    // Sanity: the build-script-generated `CHAIN_HASH` must never be
    // all zeros. An all-zero value is the placeholder we used
    // before #69 wired `sov-build`; if it shows up again, somebody
    // accidentally bypassed the build script and clients on a
    // different runtime composition could replay our transactions
    // without the chain noticing.
    use sov_modules_api::Runtime as _;
    type Rt = Runtime<TestSpec>;
    assert_ne!(Rt::CHAIN_HASH, [0u8; 32], "CHAIN_HASH is the placeholder; build.rs must run");
}

mod genesis_loader {
    use std::fs;

    use attestation::AttestationConfig;
    use sov_bank::{config_gas_token_id, TokenId};
    use sov_modules_api::Amount;
    use sov_test_utils::runtime::genesis::optimistic::{
        HighLevelOptimisticGenesisConfig, MinimalOptimisticGenesisConfig,
    };
    use tempfile::TempDir;

    use super::TestSpec;
    use crate::genesis_config::{
        create_genesis_config, validate_config, GenesisError, GenesisPaths,
    };
    use crate::runtime::GenesisConfig;

    /// Builds a [`GenesisConfig`] that is valid by construction.
    ///
    /// The 9 SDK module configs come from
    /// [`HighLevelOptimisticGenesisConfig::generate`]. The
    /// attestation config is hand-built with `lgt_token_id` set to
    /// the bank's gas-token id, satisfying [`validate_config`].
    fn happy_config() -> GenesisConfig<TestSpec> {
        type S = TestSpec;

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

        GenesisConfig::<S> {
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
        }
    }

    /// Writes each module's config to `<dir>/<module>.json` and
    /// returns the matching [`GenesisPaths`].
    fn write_to_dir(dir: &TempDir, cfg: &GenesisConfig<TestSpec>) -> GenesisPaths {
        let path = dir.path();
        for (file, json) in [
            ("bank.json", serde_json::to_vec_pretty(&cfg.bank).unwrap()),
            ("accounts.json", serde_json::to_vec_pretty(&cfg.accounts).unwrap()),
            (
                "sequencer_registry.json",
                serde_json::to_vec_pretty(&cfg.sequencer_registry).unwrap(),
            ),
            (
                "operator_incentives.json",
                serde_json::to_vec_pretty(&cfg.operator_incentives).unwrap(),
            ),
            (
                "attester_incentives.json",
                serde_json::to_vec_pretty(&cfg.attester_incentives).unwrap(),
            ),
            ("prover_incentives.json", serde_json::to_vec_pretty(&cfg.prover_incentives).unwrap()),
            ("chain_state.json", serde_json::to_vec_pretty(&cfg.chain_state).unwrap()),
            ("attestation.json", serde_json::to_vec_pretty(&cfg.attestation).unwrap()),
        ] {
            fs::write(path.join(file), json).unwrap();
        }
        GenesisPaths::from_dir(path)
    }

    #[test]
    fn validate_accepts_consistent_config() {
        let cfg = happy_config();
        validate_config(&cfg).expect("happy config should validate");
    }

    #[test]
    fn validate_rejects_missing_gas_token() {
        let mut cfg = happy_config();
        cfg.bank.gas_token_config = None;

        match validate_config(&cfg) {
            Err(GenesisError::MissingGasToken) => {}
            other => panic!("expected MissingGasToken, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_lgt_token_id_mismatch() {
        let mut cfg = happy_config();
        // Any non-default TokenId works; we just need it to differ
        // from `config_gas_token_id()`. `[7; 32]` is unrelated to
        // both the gas token's deterministic id and any token a real
        // genesis would mint.
        cfg.attestation.lgt_token_id = TokenId::from([7u8; 32]);

        match validate_config(&cfg) {
            Err(GenesisError::LgtTokenIdMismatch { attestation, bank }) => {
                assert_ne!(attestation, bank);
                assert_eq!(bank, config_gas_token_id().to_string());
            }
            other => panic!("expected LgtTokenIdMismatch, got {other:?}"),
        }
    }

    #[test]
    fn create_genesis_config_round_trips_through_json() {
        let dir = TempDir::new().unwrap();
        let original = happy_config();
        let paths = write_to_dir(&dir, &original);

        let loaded = create_genesis_config::<TestSpec>(&paths).expect("round-trip should succeed");

        // The bank's gas-token id is injected at apply-time via
        // `config_gas_token_id()`, so we can't compare bank configs
        // byte-for-byte. The cross-module invariant we *do* care
        // about is that attestation.lgt_token_id survived the JSON
        // round-trip and still matches the bank's id.
        assert_eq!(loaded.attestation.lgt_token_id, original.attestation.lgt_token_id);
        assert_eq!(loaded.attestation.lgt_token_id, config_gas_token_id());
    }

    #[test]
    fn create_genesis_config_surfaces_io_error_with_path() {
        // `from_dir` over a directory that doesn't contain the
        // expected files. We expect `Io`, not `ParseJson`, and the
        // error must carry the offending path so an operator can
        // see which file was missing.
        let dir = TempDir::new().unwrap();
        let paths = GenesisPaths::from_dir(dir.path());
        let expected_missing = dir.path().join("bank.json");

        // `GenesisConfig<S>` doesn't impl Debug, so we can't
        // `{other:?}` the whole Result; pull the error out first.
        let err = create_genesis_config::<TestSpec>(&paths)
            .err()
            .expect("missing bank.json should surface an error");
        match err {
            GenesisError::Io { path, .. } => assert_eq!(path, expected_missing),
            other => panic!("expected Io error, got {other:?}"),
        }
    }

    #[test]
    fn create_genesis_config_surfaces_parse_error_with_path() {
        // Write a malformed JSON to surface ParseJson — and make
        // sure the path in the error points at the bad file.
        let dir = TempDir::new().unwrap();
        let paths = GenesisPaths::from_dir(dir.path());

        // Write valid JSONs for everything *except* attestation.
        let cfg = happy_config();
        write_to_dir(&dir, &cfg);
        fs::write(&paths.attestation, b"{ this is not json").unwrap();

        let err = create_genesis_config::<TestSpec>(&paths)
            .err()
            .expect("malformed attestation.json should surface an error");
        match err {
            GenesisError::ParseJson { path, .. } => assert_eq!(path, paths.attestation),
            other => panic!("expected ParseJson error, got {other:?}"),
        }
    }
}
