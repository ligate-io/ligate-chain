//! End-to-end smoke test: production runtime + REST query path.
//!
//! Exercises the **production** runtime (`ligate_stf::Runtime` under
//! `MockRollupSpec` carrying `MultiAddressEvm`) wired against the
//! attestation module's auto-mounted REST surface, with state seeded
//! through the production `AttestationConfig::initial_*` genesis
//! channels. Validates four things the existing tests don't:
//!
//! 1. The production runtime instantiates under `TestRunner`. Without
//!    the `MinimalGenesis` impl in `ligate_stf::test_utils` (gated by
//!    the `test-utils` feature), this wouldn't compile.
//! 2. Genesis-time attestor-set + schema seeding actually populates
//!    state on the production runtime, not just on the macro-
//!    generated test runtime in
//!    `crates/modules/attestation/tests/integration.rs`.
//! 3. The auto-mounted module REST surface (`HasCustomRestApi` →
//!    runtime's REST router) is reachable on the production runtime
//!    under `MultiAddressEvm`. The module-level integration tests
//!    cover the same routes against a single-module `AttestationRuntime`
//!    with `TestSpec`; the production address type is a separate
//!    code path through serde.
//! 4. Both 200 and 404 paths work end-to-end against the production
//!    stack.
//!
//! ## Why genesis-seeding instead of tx submission
//!
//! TestRunner's `execute_transaction` path is incompatible with the
//! production runtime's `Auth = RollupAuthenticator<S, Self>`: the
//! `TestUser::create_plain_message` helper produces messages that
//! pass the macro-generated optimistic-runtime Auth but get silently
//! dropped under the production authenticator (empty batch receipts
//! → out-of-bounds panic on `result.batch_receipts[0]`).
//!
//! Closing that loop needs either custom Auth scaffolding for tests
//! or the binary-spawn pattern from issue #123's option (a). Both
//! are larger than this test file should swallow. Genesis-seeding
//! sidesteps the Auth path entirely while still exercising the
//! production runtime + module + REST + storage layers.

use attestation::{
    AttestationConfig, AttestorSet, AttestorSetResponse, InitialAttestorSet, InitialSchema, PubKey,
    Schema, SchemaResponse,
};
use ed25519_dalek::SigningKey;
use ligate_rollup::MockRollupSpec;
use ligate_stf::runtime::GenesisConfig;
use ligate_stf::Runtime;
use sov_bank::{config_gas_token_id, Amount};
use sov_modules_api::execution_mode::Native;
use sov_modules_stf_blueprint::GenesisParams;
use sov_test_utils::runtime::genesis::optimistic::{
    HighLevelOptimisticGenesisConfig, MinimalOptimisticGenesisConfig,
};
use sov_test_utils::runtime::{ApiPath, TestRunner};

type S = MockRollupSpec<Native>;
type RT = Runtime<S>;

/// Three deterministic ed25519 signing keys used as attestors.
fn sample_signers() -> [SigningKey; 3] {
    [
        SigningKey::from_bytes(&[1u8; 32]),
        SigningKey::from_bytes(&[2u8; 32]),
        SigningKey::from_bytes(&[3u8; 32]),
    ]
}

fn pubkeys_of(signers: &[SigningKey]) -> Vec<PubKey> {
    signers.iter().map(|sk| PubKey::from(sk.verifying_key().to_bytes())).collect()
}

/// Path builder matching the attestation module's auto-mounted REST
/// routes (see `crates/modules/attestation/src/query.rs`).
fn attestation_api_path(suffix: &str) -> ApiPath {
    ApiPath::query_module("attestation").with_custom_api_path(suffix)
}

/// Builds a `TestRunner` against the production runtime with one
/// attestor set + one schema pre-registered through the
/// `AttestationConfig::initial_*` channels. Returns the runner plus
/// the ids the test will assert against.
struct StagedGenesis {
    runner: TestRunner<RT, S>,
    attestor_set_id: attestation::AttestorSetId,
    schema_id: attestation::SchemaId,
    schema_owner: <S as sov_modules_api::Spec>::Address,
}

fn setup_with_seeded_state() -> StagedGenesis {
    // One extra account to act as the schema owner. Seeding through
    // genesis assigns the owner address explicitly (no tx sender), so
    // the address has to be a real on-chain account.
    let high_level =
        HighLevelOptimisticGenesisConfig::<S>::generate().add_accounts_with_default_balance(2);
    let schema_owner = high_level.additional_accounts()[0].address();
    let treasury = high_level.additional_accounts()[1].address();

    // Attestor set seeded at genesis. The module sorts members at
    // registration time; the derived id collapses any ordering of
    // the three deterministic signers.
    let signers = sample_signers();
    let pubs = pubkeys_of(&signers);
    let attestor_set_id = AttestorSet::derive_id(&pubs, 2);

    // Schema seeded at genesis, referencing the seeded attestor set.
    // No fee routing. Keeps the assertion surface narrow.
    let schema_name = "themisra.proof-of-prompt";
    let schema_version = 1u32;
    let schema_id = Schema::<S>::derive_id(&schema_owner, schema_name, schema_version);

    let attestation = AttestationConfig::<S> {
        treasury,
        lgt_token_id: config_gas_token_id(),
        attestation_fee: Amount::ZERO,
        schema_registration_fee: Amount::ZERO,
        attestor_set_fee: Amount::ZERO,
        initial_attestor_sets: vec![InitialAttestorSet { members: pubs.clone(), threshold: 2 }],
        initial_schemas: vec![InitialSchema {
            owner: schema_owner,
            name: schema_name.to_string(),
            version: schema_version,
            attestor_set: attestor_set_id,
            fee_routing_bps: 0,
            fee_routing_addr: None,
        }],
    };

    let minimal: MinimalOptimisticGenesisConfig<S> = high_level.into();
    let basic = minimal.config;

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

    // The production `GenesisConfig` doesn't expose
    // `into_genesis_params()` (that's a macro-generated helper on
    // `generate_optimistic_runtime!` wrappers). The wrapper itself
    // is just `GenesisParams { runtime }`.
    let runner = TestRunner::new_with_genesis(GenesisParams { runtime: genesis }, RT::default());

    StagedGenesis { runner, attestor_set_id, schema_id, schema_owner }
}

#[tokio::test(flavor = "multi_thread")]
async fn rest_api_serves_genesis_seeded_schema() {
    let mut staged = setup_with_seeded_state();
    let client = staged.runner.setup_rest_api_server().await;

    let resp: SchemaResponse<S> = staged
        .runner
        .query_api_response(
            &attestation_api_path(&format!("schemas/{}", staged.schema_id)),
            &client,
        )
        .await;

    assert_eq!(resp.schema.owner, staged.schema_owner);
    assert_eq!(resp.schema.name.as_str(), "themisra.proof-of-prompt");
    assert_eq!(resp.schema.version, 1);
    assert_eq!(resp.schema.attestor_set, staged.attestor_set_id);
}

#[tokio::test(flavor = "multi_thread")]
async fn rest_api_serves_genesis_seeded_attestor_set() {
    let mut staged = setup_with_seeded_state();
    let client = staged.runner.setup_rest_api_server().await;

    let resp: AttestorSetResponse = staged
        .runner
        .query_api_response(
            &attestation_api_path(&format!("attestor-sets/{}", staged.attestor_set_id)),
            &client,
        )
        .await;

    assert_eq!(resp.attestor_set.threshold, 2);
    let mut got: Vec<PubKey> = resp.attestor_set.members.as_slice().to_vec();
    got.sort_by_key(|pk| *pk.as_bytes());
    let mut expected = pubkeys_of(&sample_signers());
    expected.sort_by_key(|pk| *pk.as_bytes());
    assert_eq!(got, expected);
}

#[tokio::test(flavor = "multi_thread")]
async fn rest_api_returns_404_for_unknown_schema() {
    let mut staged = setup_with_seeded_state();
    let client = staged.runner.setup_rest_api_server().await;

    // Well-formed Bech32m id but no schema registered at this id.
    let unknown = attestation::SchemaId::from([0xAAu8; 32]);
    let resp = staged
        .runner
        .query_api(&attestation_api_path(&format!("schemas/{unknown}")), &client)
        .await;

    assert_eq!(resp.status().as_u16(), 404);
}

#[tokio::test(flavor = "multi_thread")]
async fn rest_api_returns_404_for_unknown_attestation() {
    // No tx submission means no `Attestation` was written; every
    // attestation lookup must 404. Locks in the contract that
    // genesis seeding handles attestor sets + schemas only, not
    // attestations.
    let mut staged = setup_with_seeded_state();
    let client = staged.runner.setup_rest_api_server().await;

    let unknown_payload = attestation::PayloadHash::from([0x42u8; 32]);
    let attestation_id = attestation::AttestationId::from_pair(&staged.schema_id, &unknown_payload);

    let resp = staged
        .runner
        .query_api(&attestation_api_path(&format!("attestations/{attestation_id}")), &client)
        .await;

    assert_eq!(resp.status().as_u16(), 404);
}

#[tokio::test(flavor = "multi_thread")]
async fn rest_api_returns_400_for_invalid_bech32() {
    let mut staged = setup_with_seeded_state();
    let client = staged.runner.setup_rest_api_server().await;

    let resp =
        staged.runner.query_api(&attestation_api_path("schemas/not-a-valid-id"), &client).await;

    assert_eq!(resp.status().as_u16(), 400);
}
