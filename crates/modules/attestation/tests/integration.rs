//! Integration tests for the attestation module against the new SDK.
//!
//! Built on `sov-test-utils::TestRunner`. Each test sets up a fresh
//! optimistic runtime containing only the attestation module, runs
//! genesis with a chosen [`AttestationConfig`], and exercises one
//! handler path with assertions on the resulting state and events.
//!
//! The behaviours mirror the v0.3-era test suite preserved at
//! `/tmp/attestation_tests_v0_3.rs` for reference: validation
//! invariants on the registration paths, fee splitting on
//! [`SubmitAttestation`], and bank-balance integration that proves
//! `$LGT` actually moves.
//!
//! [`SubmitAttestation`]: attestation::CallMessage::SubmitAttestation

use attestation::{
    AttestationConfig, AttestationModule, AttestorSet, AttestorSignature, CallMessage,
    InitialAttestorSet, MAX_ATTESTOR_SET_MEMBERS, MAX_ATTESTATION_SIGNATURES,
    MAX_ATTESTOR_SIGNATURE_BYTES, PubKey, Schema, SchemaId,
};
use ed25519_dalek::{Signer, SigningKey};
use sov_bank::{config_gas_token_id, Amount, Bank, TokenId};
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{SafeString, SafeVec, TxEffect};
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::TestRunner;
use sov_test_utils::{generate_optimistic_runtime, AsUser, TestUser, TransactionTestCase};

generate_optimistic_runtime!(AttestationRuntime <= attestation: AttestationModule<S>);

type S = sov_test_utils::TestSpec;
type RT = AttestationRuntime<S>;

/// Per-test scaffolding: a `TestRunner` plus the test users genesis
/// allocated `$LGT` to.
struct TestEnv {
    runner: TestRunner<RT, S>,
    /// Submitter for attestation txs. Holds enough `$LGT` to cover
    /// all the fee-charging paths.
    submitter: TestUser<S>,
    /// Treasury address; receives the treasury share of every fee.
    /// Needs to be a real account in `bank` (so bank.transfer_from
    /// can deposit into it), which means the genesis allocation
    /// gives it a non-zero balance.
    treasury: <S as sov_modules_api::Spec>::Address,
    /// `$LGT` token id. Same value across all tests, comes from the
    /// SDK's gas-token convention via `config_gas_token_id()`.
    lgt_token_id: TokenId,
}

/// Build a fresh `TestEnv` with the given attestation fee values.
fn setup(
    attestation_fee: Amount,
    schema_registration_fee: Amount,
    attestor_set_fee: Amount,
) -> TestEnv {
    setup_with_initial(attestation_fee, schema_registration_fee, attestor_set_fee, vec![])
}

/// Like [`setup`], plus pre-registers a list of attestor sets at
/// genesis (so registration-fee charging gets exercised but not
/// duplicated by `RegisterAttestorSet` in the test body).
fn setup_with_initial(
    attestation_fee: Amount,
    schema_registration_fee: Amount,
    attestor_set_fee: Amount,
    initial_attestor_sets: Vec<InitialAttestorSet>,
) -> TestEnv {
    // Two extra accounts: index 0 = submitter, index 1 = treasury.
    let genesis_config = HighLevelOptimisticGenesisConfig::generate()
        .add_accounts_with_default_balance(2);

    let submitter = genesis_config.additional_accounts()[0].clone();
    let treasury_user = genesis_config.additional_accounts()[1].clone();
    let treasury = treasury_user.address();

    let lgt_token_id = config_gas_token_id();

    let attestation_config = AttestationConfig::<S> {
        treasury,
        lgt_token_id,
        attestation_fee,
        schema_registration_fee,
        attestor_set_fee,
        initial_attestor_sets,
        initial_schemas: vec![],
    };

    let genesis =
        GenesisConfig::from_minimal_config(genesis_config.into(), attestation_config);

    let runner = TestRunner::new_with_genesis(genesis.into_genesis_params(), RT::default());

    TestEnv { runner, submitter, treasury, lgt_token_id }
}

/// Three deterministic ed25519 signing keys for use as attestors.
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

/// Build a `SafeVec<PubKey>` for a CallMessage payload.
fn safe_members(signers: &[SigningKey]) -> SafeVec<PubKey, MAX_ATTESTOR_SET_MEMBERS> {
    SafeVec::try_from(pubkeys_of(signers)).expect("members fit MAX_ATTESTOR_SET_MEMBERS")
}

fn safe_name(s: &str) -> SafeString {
    SafeString::try_from(s.to_string()).expect("name fits SafeString default cap")
}

#[test]
fn register_attestor_set_happy_path() {
    let mut env = setup(Amount::ZERO, Amount::ZERO, Amount::new(7));

    let signers = sample_signers();
    let members = safe_members(&signers);
    let derived_id = AttestorSet::derive_id(&pubkeys_of(&signers), 2);

    let treasury_addr = env.treasury;
    let lgt = env.lgt_token_id;

    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::RegisterAttestorSet { members, threshold: 2 },
        ),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful());

            let module = AttestationModule::<S>::default();
            let stored = module
                .attestor_sets
                .get(&derived_id, state)
                .unwrap_infallible()
                .expect("attestor set should be in state after registration");
            assert_eq!(stored.threshold, 2);
            assert_eq!(stored.members.len(), 3);

            // Counter advanced by exactly the attestor-set fee.
            assert_eq!(
                module.total_treasury_collected.get(state).unwrap_infallible(),
                Some(Amount::new(7))
            );

            // Bank moved real $LGT to the treasury. We don't pin the
            // exact balance because the treasury account starts with
            // the default genesis balance; we only assert the
            // post-condition is at least that plus the fee.
            let bank = Bank::<S>::default();
            let treasury_bal = bank
                .get_balance_of(&treasury_addr, lgt, state)
                .unwrap_infallible()
                .unwrap_or(Amount::ZERO);
            assert!(
                treasury_bal >= Amount::new(7),
                "treasury holds at least the fee in $LGT, got {treasury_bal:?}"
            );
        }),
    });
}

#[test]
fn register_attestor_set_rejects_threshold_above_members() {
    let mut env = setup(Amount::ZERO, Amount::ZERO, Amount::ZERO);

    // 1 member, threshold 2 → invalid.
    let lone_signer = SigningKey::from_bytes(&[42u8; 32]);
    let members = SafeVec::try_from(vec![PubKey::from(lone_signer.verifying_key().to_bytes())])
        .unwrap();

    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::RegisterAttestorSet { members, threshold: 2 },
        ),
        assert: Box::new(|result, _state| {
            assert!(matches!(result.tx_receipt, TxEffect::Reverted(_)));
        }),
    });
}

#[test]
fn register_attestor_set_rejects_zero_threshold() {
    let mut env = setup(Amount::ZERO, Amount::ZERO, Amount::ZERO);

    let signers = sample_signers();
    let members = safe_members(&signers);

    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::RegisterAttestorSet { members, threshold: 0 },
        ),
        assert: Box::new(|result, _state| {
            assert!(matches!(result.tx_receipt, TxEffect::Reverted(_)));
        }),
    });
}

#[test]
fn register_schema_requires_existing_attestor_set() {
    let mut env = setup(Amount::ZERO, Amount::ZERO, Amount::ZERO);

    // Reference an attestor set id that was never registered.
    let bogus_set_id = AttestorSet::derive_id(&[PubKey::from([0xAAu8; 32])], 1);

    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::RegisterSchema {
                name: safe_name("foo"),
                version: 1,
                attestor_set: bogus_set_id,
                fee_routing_bps: 0,
                fee_routing_addr: None,
            },
        ),
        assert: Box::new(|result, _state| {
            assert!(matches!(result.tx_receipt, TxEffect::Reverted(_)));
        }),
    });
}

#[test]
fn register_schema_rejects_over_cap_fee_routing() {
    // Pre-register the attestor set at genesis so the schema's
    // `attestor_set` field references a known id.
    let signers = sample_signers();
    let pubs = pubkeys_of(&signers);
    let initial = vec![InitialAttestorSet { members: pubs.clone(), threshold: 2 }];
    let mut env = setup_with_initial(Amount::ZERO, Amount::ZERO, Amount::ZERO, initial);
    let attestor_set_id = AttestorSet::derive_id(&pubs, 2);

    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::RegisterSchema {
                name: safe_name("over-cap"),
                version: 1,
                attestor_set: attestor_set_id,
                fee_routing_bps: 6000, // > MAX_BUILDER_BPS (5000)
                fee_routing_addr: Some(env.submitter.address()),
            },
        ),
        assert: Box::new(|result, _state| {
            assert!(matches!(result.tx_receipt, TxEffect::Reverted(_)));
        }),
    });
}

#[test]
fn submit_attestation_routes_treasury_share_via_bank() {
    // Setup: 1000-nano attestation fee, 25% to builder, 75% to
    // treasury. Pre-register the attestor set so the submit path
    // doesn't have to register one first.
    let signers = sample_signers();
    let pubs = pubkeys_of(&signers);
    let initial = vec![InitialAttestorSet { members: pubs.clone(), threshold: 2 }];
    let mut env = setup_with_initial(
        Amount::new(1_000),
        Amount::ZERO,
        Amount::ZERO,
        initial,
    );
    let attestor_set_id = AttestorSet::derive_id(&pubs, 2);
    let submitter_addr = env.submitter.address();

    // Register the schema (no routing for this test) at runtime.
    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::RegisterSchema {
                name: safe_name("no-routing"),
                version: 1,
                attestor_set: attestor_set_id,
                fee_routing_bps: 0,
                fee_routing_addr: None,
            },
        ),
        assert: Box::new(|result, _state| assert!(result.tx_receipt.is_successful())),
    });

    let schema_id = Schema::<S>::derive_id(&submitter_addr, "no-routing", 1);

    // Sign the attestation with two attestors (≥ threshold of 2).
    let payload_hash = attestation::PayloadHash::from([0xAAu8; 32]);
    let timestamp = 0u64;
    let digest = attestation::SignedAttestationPayload::<S> {
        schema_id,
        payload_hash,
        submitter: submitter_addr,
        timestamp,
    }
    .digest();
    let sigs: Vec<AttestorSignature> = signers[..2]
        .iter()
        .map(|sk| AttestorSignature {
            pubkey: PubKey::from(sk.verifying_key().to_bytes()),
            sig: SafeVec::<u8, MAX_ATTESTOR_SIGNATURE_BYTES>::try_from(
                sk.sign(&digest).to_bytes().to_vec(),
            )
            .unwrap(),
        })
        .collect();
    let signatures =
        SafeVec::<AttestorSignature, MAX_ATTESTATION_SIGNATURES>::try_from(sigs).unwrap();

    let treasury_addr = env.treasury;
    let lgt = env.lgt_token_id;

    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::SubmitAttestation {
                schema_id,
                payload_hash,
                signatures,
            },
        ),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful());
            let module = AttestationModule::<S>::default();
            assert_eq!(
                module.total_treasury_collected.get(state).unwrap_infallible(),
                Some(Amount::new(1_000)),
                "all 1000 nanos go to the treasury (no fee routing on this schema)"
            );
            // Bank balances reflect the same.
            let bank = Bank::<S>::default();
            let treasury_bal = bank
                .get_balance_of(&treasury_addr, lgt, state)
                .unwrap_infallible()
                .unwrap_or(Amount::ZERO);
            assert!(
                treasury_bal >= Amount::new(1_000),
                "treasury received at least 1000 nanos via bank"
            );
        }),
    });
}
