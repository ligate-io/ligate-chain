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
    InitialAttestorSet, PubKey, Schema, MAX_ATTESTATION_SIGNATURES, MAX_ATTESTOR_SET_MEMBERS,
    MAX_ATTESTOR_SIGNATURE_BYTES,
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
    let genesis_config =
        HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(2);

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

    let genesis = GenesisConfig::from_minimal_config(genesis_config.into(), attestation_config);

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
    let members =
        SafeVec::try_from(vec![PubKey::from(lone_signer.verifying_key().to_bytes())]).unwrap();

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
    let mut env = setup_with_initial(Amount::new(1_000), Amount::ZERO, Amount::ZERO, initial);
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
            CallMessage::SubmitAttestation { schema_id, payload_hash, signatures },
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

// ============================================================================
// Schema validation reject paths
// ============================================================================

#[test]
fn register_schema_rejects_orphan_routing_bps_without_addr() {
    let signers = sample_signers();
    let pubs = pubkeys_of(&signers);
    let initial = vec![InitialAttestorSet { members: pubs.clone(), threshold: 2 }];
    let mut env = setup_with_initial(Amount::ZERO, Amount::ZERO, Amount::ZERO, initial);
    let attestor_set_id = AttestorSet::derive_id(&pubs, 2);

    // bps > 0 with no addr → rejected.
    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::RegisterSchema {
                name: safe_name("orphan-bps"),
                version: 1,
                attestor_set: attestor_set_id,
                fee_routing_bps: 1000,
                fee_routing_addr: None,
            },
        ),
        assert: Box::new(|result, _state| {
            assert!(matches!(result.tx_receipt, TxEffect::Reverted(_)));
        }),
    });
}

#[test]
fn register_schema_rejects_orphan_routing_addr_without_bps() {
    let signers = sample_signers();
    let pubs = pubkeys_of(&signers);
    let initial = vec![InitialAttestorSet { members: pubs.clone(), threshold: 2 }];
    let mut env = setup_with_initial(Amount::ZERO, Amount::ZERO, Amount::ZERO, initial);
    let attestor_set_id = AttestorSet::derive_id(&pubs, 2);
    let routing_addr = env.submitter.address();

    // addr set but bps == 0 → rejected.
    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::RegisterSchema {
                name: safe_name("orphan-addr"),
                version: 1,
                attestor_set: attestor_set_id,
                fee_routing_bps: 0,
                fee_routing_addr: Some(routing_addr),
            },
        ),
        assert: Box::new(|result, _state| {
            assert!(matches!(result.tx_receipt, TxEffect::Reverted(_)));
        }),
    });
}

#[test]
fn submit_attestation_requires_existing_schema() {
    let mut env = setup(Amount::ZERO, Amount::ZERO, Amount::ZERO);

    // Reference a schema id that was never registered.
    let bogus_schema = attestation::SchemaId::from([0xDEu8; 32]);
    let bogus_payload = attestation::PayloadHash::from([0xADu8; 32]);

    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::SubmitAttestation {
                schema_id: bogus_schema,
                payload_hash: bogus_payload,
                signatures: SafeVec::try_from(Vec::<AttestorSignature>::new()).unwrap(),
            },
        ),
        assert: Box::new(|result, _state| {
            assert!(matches!(result.tx_receipt, TxEffect::Reverted(_)));
        }),
    });
}

// ============================================================================
// Full register-register-submit flow + write-once invariant
// ============================================================================

#[test]
fn full_register_register_submit_flow() {
    let mut env = setup(Amount::ZERO, Amount::ZERO, Amount::ZERO);
    let signers = sample_signers();
    let submitter_addr = env.submitter.address();

    // 1. Register attestor set (threshold 2 of 3).
    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::RegisterAttestorSet { members: safe_members(&signers), threshold: 2 },
        ),
        assert: Box::new(|result, _state| assert!(result.tx_receipt.is_successful())),
    });
    let attestor_set_id = AttestorSet::derive_id(&pubkeys_of(&signers), 2);

    // 2. Register schema.
    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::RegisterSchema {
                name: safe_name("themisra.proof-of-prompt"),
                version: 1,
                attestor_set: attestor_set_id,
                fee_routing_bps: 0,
                fee_routing_addr: None,
            },
        ),
        assert: Box::new(|result, _state| assert!(result.tx_receipt.is_successful())),
    });
    let schema_id = Schema::<S>::derive_id(&submitter_addr, "themisra.proof-of-prompt", 1);

    // 3. Submit attestation signed by 2 of 3 (meets threshold).
    let payload_hash = attestation::PayloadHash::from([0x42u8; 32]);
    let digest = attestation::SignedAttestationPayload::<S> {
        schema_id,
        payload_hash,
        submitter: submitter_addr,
        timestamp: 0,
    }
    .digest();
    let sig_vec: Vec<AttestorSignature> = signers[..2]
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
        SafeVec::<AttestorSignature, MAX_ATTESTATION_SIGNATURES>::try_from(sig_vec.clone())
            .unwrap();

    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::SubmitAttestation { schema_id, payload_hash, signatures },
        ),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful());
            // 4. Verify state.
            let module = AttestationModule::<S>::default();
            let key = attestation::AttestationId::from_pair(&schema_id, &payload_hash);
            let stored = module
                .attestations
                .get(&key, state)
                .unwrap_infallible()
                .expect("attestation should be in state");
            assert_eq!(stored.schema_id, schema_id);
            assert_eq!(stored.payload_hash, payload_hash);
            assert_eq!(stored.submitter, submitter_addr);
        }),
    });

    // 5. Write-once: resubmitting same (schema_id, payload_hash) must fail.
    let signatures_again =
        SafeVec::<AttestorSignature, MAX_ATTESTATION_SIGNATURES>::try_from(sig_vec).unwrap();
    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::SubmitAttestation {
                schema_id,
                payload_hash,
                signatures: signatures_again,
            },
        ),
        assert: Box::new(|result, _state| {
            assert!(matches!(result.tx_receipt, TxEffect::Reverted(_)));
        }),
    });
}

// ============================================================================
// Signature validation reject paths
// ============================================================================

/// Sets up an env + registers attestor set + registers schema, returns
/// the schema id and the digest the attestors must sign over.
fn sig_test_setup(
    env: &mut TestEnv,
    signers: &[SigningKey; 3],
    schema_name: &str,
    payload_hash: attestation::PayloadHash,
) -> (attestation::SchemaId, attestation::Hash32) {
    let submitter_addr = env.submitter.address();
    let members = safe_members(signers);
    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::RegisterAttestorSet { members, threshold: 2 },
        ),
        assert: Box::new(|result, _state| assert!(result.tx_receipt.is_successful())),
    });
    let attestor_set_id = AttestorSet::derive_id(&pubkeys_of(signers), 2);

    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::RegisterSchema {
                name: safe_name(schema_name),
                version: 1,
                attestor_set: attestor_set_id,
                fee_routing_bps: 0,
                fee_routing_addr: None,
            },
        ),
        assert: Box::new(|result, _state| assert!(result.tx_receipt.is_successful())),
    });
    let schema_id = Schema::<S>::derive_id(&submitter_addr, schema_name, 1);
    let digest = attestation::SignedAttestationPayload::<S> {
        schema_id,
        payload_hash,
        submitter: submitter_addr,
        timestamp: 0,
    }
    .digest();
    (schema_id, digest)
}

fn sig_for(sk: &SigningKey, digest: &attestation::Hash32) -> AttestorSignature {
    AttestorSignature {
        pubkey: PubKey::from(sk.verifying_key().to_bytes()),
        sig: SafeVec::<u8, MAX_ATTESTOR_SIGNATURE_BYTES>::try_from(
            sk.sign(digest).to_bytes().to_vec(),
        )
        .unwrap(),
    }
}

#[test]
fn submit_below_threshold_rejects() {
    let mut env = setup(Amount::ZERO, Amount::ZERO, Amount::ZERO);
    let signers = sample_signers();
    let payload_hash = attestation::PayloadHash::from([0x01u8; 32]);
    let (schema_id, digest) = sig_test_setup(&mut env, &signers, "below-threshold", payload_hash);

    // Only 1 signature, threshold is 2.
    let signatures = SafeVec::try_from(vec![sig_for(&signers[0], &digest)]).unwrap();

    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::SubmitAttestation { schema_id, payload_hash, signatures },
        ),
        assert: Box::new(|result, _state| {
            assert!(matches!(result.tx_receipt, TxEffect::Reverted(_)));
        }),
    });
}

#[test]
fn submit_with_unknown_pubkey_rejects() {
    let mut env = setup(Amount::ZERO, Amount::ZERO, Amount::ZERO);
    let signers = sample_signers();
    let payload_hash = attestation::PayloadHash::from([0x02u8; 32]);
    let (schema_id, digest) = sig_test_setup(&mut env, &signers, "unknown-pubkey", payload_hash);

    // One legit sig + one signed by a stranger NOT in the attestor set.
    let stranger = SigningKey::from_bytes(&[99u8; 32]);
    let signatures =
        SafeVec::try_from(vec![sig_for(&signers[0], &digest), sig_for(&stranger, &digest)])
            .unwrap();

    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::SubmitAttestation { schema_id, payload_hash, signatures },
        ),
        assert: Box::new(|result, _state| {
            assert!(matches!(result.tx_receipt, TxEffect::Reverted(_)));
        }),
    });
}

#[test]
fn submit_with_invalid_signature_rejects() {
    let mut env = setup(Amount::ZERO, Amount::ZERO, Amount::ZERO);
    let signers = sample_signers();
    let payload_hash = attestation::PayloadHash::from([0x03u8; 32]);
    let (schema_id, digest) = sig_test_setup(&mut env, &signers, "invalid-sig", payload_hash);

    // Valid sig + one whose bytes are corrupted (flip a bit).
    let mut corrupted_bytes = signers[0].sign(&digest).to_bytes().to_vec();
    corrupted_bytes[0] ^= 0x01;
    let corrupted = AttestorSignature {
        pubkey: PubKey::from(signers[0].verifying_key().to_bytes()),
        sig: SafeVec::try_from(corrupted_bytes).unwrap(),
    };
    let signatures = SafeVec::try_from(vec![sig_for(&signers[1], &digest), corrupted]).unwrap();

    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::SubmitAttestation { schema_id, payload_hash, signatures },
        ),
        assert: Box::new(|result, _state| {
            assert!(matches!(result.tx_receipt, TxEffect::Reverted(_)));
        }),
    });
}

#[test]
fn submit_with_duplicate_pubkey_rejects() {
    let mut env = setup(Amount::ZERO, Amount::ZERO, Amount::ZERO);
    let signers = sample_signers();
    let payload_hash = attestation::PayloadHash::from([0x04u8; 32]);
    let (schema_id, digest) = sig_test_setup(&mut env, &signers, "duplicate-pubkey", payload_hash);

    // Same signer twice, both technically valid.
    let s = sig_for(&signers[0], &digest);
    let signatures = SafeVec::try_from(vec![s.clone(), s]).unwrap();

    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::SubmitAttestation { schema_id, payload_hash, signatures },
        ),
        assert: Box::new(|result, _state| {
            assert!(matches!(result.tx_receipt, TxEffect::Reverted(_)));
        }),
    });
}

#[test]
fn submit_with_bad_sig_length_rejects() {
    let mut env = setup(Amount::ZERO, Amount::ZERO, Amount::ZERO);
    let signers = sample_signers();
    let payload_hash = attestation::PayloadHash::from([0x05u8; 32]);
    let (schema_id, digest) = sig_test_setup(&mut env, &signers, "bad-len", payload_hash);

    // Take a legit sig but truncate the raw bytes to 63 (ed25519 expects 64).
    let mut truncated_bytes = signers[0].sign(&digest).to_bytes().to_vec();
    truncated_bytes.truncate(63);
    let truncated = AttestorSignature {
        pubkey: PubKey::from(signers[0].verifying_key().to_bytes()),
        sig: SafeVec::try_from(truncated_bytes).unwrap(),
    };
    let signatures = SafeVec::try_from(vec![sig_for(&signers[1], &digest), truncated]).unwrap();

    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::SubmitAttestation { schema_id, payload_hash, signatures },
        ),
        assert: Box::new(|result, _state| {
            assert!(matches!(result.tx_receipt, TxEffect::Reverted(_)));
        }),
    });
}

#[test]
fn submit_digest_is_bound_to_submitter() {
    // Signing the wrong submitter produces signatures that don't verify
    // against the module-computed digest. Locks in the invariant that
    // submitter is part of what attestors sign, not a caller-controlled
    // free variable.
    use sov_modules_api::Address;
    let mut env = setup(Amount::ZERO, Amount::ZERO, Amount::ZERO);
    let signers = sample_signers();
    let payload_hash = attestation::PayloadHash::from([0x06u8; 32]);
    let submitter_addr = env.submitter.address();

    let members = safe_members(&signers);
    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::RegisterAttestorSet { members, threshold: 2 },
        ),
        assert: Box::new(|result, _state| assert!(result.tx_receipt.is_successful())),
    });
    let attestor_set_id = AttestorSet::derive_id(&pubkeys_of(&signers), 2);

    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::RegisterSchema {
                name: safe_name("digest-binding"),
                version: 1,
                attestor_set: attestor_set_id,
                fee_routing_bps: 0,
                fee_routing_addr: None,
            },
        ),
        assert: Box::new(|result, _state| assert!(result.tx_receipt.is_successful())),
    });
    let schema_id = Schema::<S>::derive_id(&submitter_addr, "digest-binding", 1);

    // Sign over a digest that uses a DIFFERENT submitter address.
    let wrong_submitter: <S as sov_modules_api::Spec>::Address = Address::from([0xFFu8; 28]);
    let wrong_digest = attestation::SignedAttestationPayload::<S> {
        schema_id,
        payload_hash,
        submitter: wrong_submitter,
        timestamp: 0,
    }
    .digest();
    let signatures = SafeVec::try_from(vec![
        sig_for(&signers[0], &wrong_digest),
        sig_for(&signers[1], &wrong_digest),
    ])
    .unwrap();

    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::SubmitAttestation { schema_id, payload_hash, signatures },
        ),
        assert: Box::new(|result, _state| {
            assert!(
                matches!(result.tx_receipt, TxEffect::Reverted(_)),
                "wrong-submitter digest must not verify"
            );
        }),
    });
}

// ============================================================================
// Genesis happy path (rejection paths covered by handler tests above)
// ============================================================================

#[test]
fn genesis_seeds_initial_attestor_set() {
    // Pre-register an attestor set at genesis. The runtime should
    // expose it without any RegisterAttestorSet tx.
    let signers = sample_signers();
    let pubs = pubkeys_of(&signers);
    let initial = vec![InitialAttestorSet { members: pubs.clone(), threshold: 2 }];
    let mut env = setup_with_initial(Amount::ZERO, Amount::ZERO, Amount::ZERO, initial);
    let attestor_set_id = AttestorSet::derive_id(&pubs, 2);
    let submitter_addr = env.submitter.address();

    // Try to register a schema referencing the genesis-seeded set.
    // If genesis didn't seed it, this would fail with UnknownAttestorSet.
    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::RegisterSchema {
                name: safe_name("genesis-set-test"),
                version: 1,
                attestor_set: attestor_set_id,
                fee_routing_bps: 0,
                fee_routing_addr: None,
            },
        ),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful());
            // Verify both the genesis-seeded set and the just-registered schema are in state.
            let module = AttestationModule::<S>::default();
            assert!(module
                .attestor_sets
                .get(&attestor_set_id, state)
                .unwrap_infallible()
                .is_some());
            let schema_id = Schema::<S>::derive_id(&submitter_addr, "genesis-set-test", 1);
            assert!(module.schemas.get(&schema_id, state).unwrap_infallible().is_some());
        }),
    });
}

// ============================================================================
// Fee tests: builder routing + insufficient balance
// ============================================================================

#[test]
fn submit_attestation_routes_builder_share_via_bank() {
    // 25% routing → 250 builder + 750 treasury on a 1000-nano fee.
    let signers = sample_signers();
    let pubs = pubkeys_of(&signers);
    let initial = vec![InitialAttestorSet { members: pubs.clone(), threshold: 2 }];
    let mut env = setup_with_initial(Amount::new(1_000), Amount::ZERO, Amount::ZERO, initial);
    let attestor_set_id = AttestorSet::derive_id(&pubs, 2);
    let submitter_addr = env.submitter.address();
    let builder_addr = env.submitter.address(); // route to self for simplicity

    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::RegisterSchema {
                name: safe_name("builder-routing"),
                version: 1,
                attestor_set: attestor_set_id,
                fee_routing_bps: 2500,
                fee_routing_addr: Some(builder_addr),
            },
        ),
        assert: Box::new(|result, _state| assert!(result.tx_receipt.is_successful())),
    });
    let schema_id = Schema::<S>::derive_id(&submitter_addr, "builder-routing", 1);

    let payload_hash = attestation::PayloadHash::from([0xBBu8; 32]);
    let digest = attestation::SignedAttestationPayload::<S> {
        schema_id,
        payload_hash,
        submitter: submitter_addr,
        timestamp: 0,
    }
    .digest();
    let sigs: Vec<AttestorSignature> = signers[..2].iter().map(|sk| sig_for(sk, &digest)).collect();
    let signatures = SafeVec::try_from(sigs).unwrap();

    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::SubmitAttestation { schema_id, payload_hash, signatures },
        ),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful());
            let module = AttestationModule::<S>::default();

            // Counter accumulators reflect the split.
            assert_eq!(
                module.total_treasury_collected.get(state).unwrap_infallible(),
                Some(Amount::new(750))
            );
            assert_eq!(
                module.builder_fees_collected.get(&builder_addr, state).unwrap_infallible(),
                Some(Amount::new(250))
            );
        }),
    });
}

#[test]
fn submit_attestation_fails_with_insufficient_balance() {
    // Set the attestation fee to absurdly higher than the submitter
    // could ever hold from genesis. Bank's `transfer_from` will return
    // Err and the tx reverts with no state changes.
    let signers = sample_signers();
    let pubs = pubkeys_of(&signers);
    let initial = vec![InitialAttestorSet { members: pubs.clone(), threshold: 2 }];
    // 10^30 nanos — guaranteed larger than any test user's balance.
    let absurd_fee = Amount::new(1_000_000_000_000_000_000_000_000_000_000);
    let mut env = setup_with_initial(absurd_fee, Amount::ZERO, Amount::ZERO, initial);
    let attestor_set_id = AttestorSet::derive_id(&pubs, 2);
    let submitter_addr = env.submitter.address();

    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::RegisterSchema {
                name: safe_name("insufficient"),
                version: 1,
                attestor_set: attestor_set_id,
                fee_routing_bps: 0,
                fee_routing_addr: None,
            },
        ),
        assert: Box::new(|result, _state| assert!(result.tx_receipt.is_successful())),
    });
    let schema_id = Schema::<S>::derive_id(&submitter_addr, "insufficient", 1);

    let payload_hash = attestation::PayloadHash::from([0xCCu8; 32]);
    let digest = attestation::SignedAttestationPayload::<S> {
        schema_id,
        payload_hash,
        submitter: submitter_addr,
        timestamp: 0,
    }
    .digest();
    let sigs: Vec<AttestorSignature> = signers[..2].iter().map(|sk| sig_for(sk, &digest)).collect();
    let signatures = SafeVec::try_from(sigs).unwrap();

    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::SubmitAttestation { schema_id, payload_hash, signatures },
        ),
        assert: Box::new(move |result, state| {
            assert!(
                matches!(result.tx_receipt, TxEffect::Reverted(_)),
                "insufficient balance should revert: {:?}",
                result.tx_receipt
            );
            // Attestation must not have been written.
            let module = AttestationModule::<S>::default();
            let key = attestation::AttestationId::from_pair(&schema_id, &payload_hash);
            assert!(module.attestations.get(&key, state).unwrap_infallible().is_none());
        }),
    });
}

// ============================================================================
// Idempotency / duplicate-prevention reject paths
// ============================================================================

#[test]
fn register_attestor_set_rejects_duplicate() {
    // Same `(members, threshold)` derives the same `AttestorSetId`.
    // Re-registering must be rejected even if all the validation
    // invariants pass; the storage layer is write-once per id.
    let mut env = setup(Amount::ZERO, Amount::ZERO, Amount::ZERO);
    let signers = sample_signers();

    // First registration succeeds.
    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::RegisterAttestorSet { members: safe_members(&signers), threshold: 2 },
        ),
        assert: Box::new(|result, _state| assert!(result.tx_receipt.is_successful())),
    });

    // Second registration with the same members + threshold reverts.
    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::RegisterAttestorSet { members: safe_members(&signers), threshold: 2 },
        ),
        assert: Box::new(|result, _state| {
            assert!(
                matches!(result.tx_receipt, TxEffect::Reverted(_)),
                "duplicate attestor set must revert: {:?}",
                result.tx_receipt
            );
        }),
    });
}

#[test]
fn register_schema_rejects_duplicate() {
    // Same `(owner, name, version)` derives the same `SchemaId`. The
    // `owner` is the tx sender; we use the same submitter for both
    // calls so the derived id collides on the second attempt.
    let signers = sample_signers();
    let pubs = pubkeys_of(&signers);
    let initial = vec![InitialAttestorSet { members: pubs.clone(), threshold: 2 }];
    let mut env = setup_with_initial(Amount::ZERO, Amount::ZERO, Amount::ZERO, initial);
    let attestor_set_id = AttestorSet::derive_id(&pubs, 2);

    // First registration succeeds.
    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::RegisterSchema {
                name: safe_name("dup-schema"),
                version: 1,
                attestor_set: attestor_set_id,
                fee_routing_bps: 0,
                fee_routing_addr: None,
            },
        ),
        assert: Box::new(|result, _state| assert!(result.tx_receipt.is_successful())),
    });

    // Second registration with the same name + version + (implicit
    // same owner from the same submitter) reverts.
    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::RegisterSchema {
                name: safe_name("dup-schema"),
                version: 1,
                attestor_set: attestor_set_id,
                fee_routing_bps: 0,
                fee_routing_addr: None,
            },
        ),
        assert: Box::new(|result, _state| {
            assert!(
                matches!(result.tx_receipt, TxEffect::Reverted(_)),
                "duplicate schema must revert: {:?}",
                result.tx_receipt
            );
        }),
    });
}

#[test]
fn register_attestor_set_rejects_empty_members() {
    // Distinct from "threshold > members.len()" with 1 member.
    // Empty members is its own validation branch (`EmptyAttestorSet`)
    // because a zero-member set with threshold 0 would otherwise sneak
    // past the threshold check.
    let mut env = setup(Amount::ZERO, Amount::ZERO, Amount::ZERO);

    let empty: SafeVec<PubKey, MAX_ATTESTOR_SET_MEMBERS> =
        SafeVec::try_from(Vec::<PubKey>::new()).unwrap();

    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::RegisterAttestorSet { members: empty, threshold: 1 },
        ),
        assert: Box::new(|result, _state| {
            assert!(
                matches!(result.tx_receipt, TxEffect::Reverted(_)),
                "empty members must revert: {:?}",
                result.tx_receipt
            );
        }),
    });
}

#[test]
fn submit_attestation_rejects_empty_signatures() {
    // Zero signatures hits `BelowThreshold` rather than any of the
    // signature-validation errors. Confirms the validator handles the
    // "no signatures at all" edge cleanly without panicking on an
    // empty iterator.
    let signers = sample_signers();
    let pubs = pubkeys_of(&signers);
    let initial = vec![InitialAttestorSet { members: pubs.clone(), threshold: 2 }];
    let mut env = setup_with_initial(Amount::ZERO, Amount::ZERO, Amount::ZERO, initial);
    let attestor_set_id = AttestorSet::derive_id(&pubs, 2);
    let submitter_addr = env.submitter.address();

    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::RegisterSchema {
                name: safe_name("empty-sigs"),
                version: 1,
                attestor_set: attestor_set_id,
                fee_routing_bps: 0,
                fee_routing_addr: None,
            },
        ),
        assert: Box::new(|result, _state| assert!(result.tx_receipt.is_successful())),
    });
    let schema_id = Schema::<S>::derive_id(&submitter_addr, "empty-sigs", 1);

    let payload_hash = attestation::PayloadHash::from([0xEEu8; 32]);
    let empty_sigs: SafeVec<AttestorSignature, MAX_ATTESTATION_SIGNATURES> =
        SafeVec::try_from(Vec::<AttestorSignature>::new()).unwrap();

    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::SubmitAttestation { schema_id, payload_hash, signatures: empty_sigs },
        ),
        assert: Box::new(|result, _state| {
            assert!(
                matches!(result.tx_receipt, TxEffect::Reverted(_)),
                "empty signatures must revert: {:?}",
                result.tx_receipt
            );
        }),
    });
}

// ============================================================================
// REST query API (GET /modules/attestation/...)
// ============================================================================
//
// These exercise the routes wired up in `attestation::query` via the
// SDK's `TestRunner::setup_rest_api_server`. The runtime auto-mounts
// every module's `HasCustomRestApi` impl under `/modules/<name>/`, so
// hitting `/modules/attestation/schemas/<id>` invokes the route in
// `query.rs` against live runtime state.
//
// Each test follows the pattern:
//   1. setup() + register attestor set + register schema (+ optionally
//      submit attestation) via tx executions, exactly like the call-
//      handler integration tests above.
//   2. setup_rest_api_server().await to start an axum server on a
//      random port and obtain a reqwest::Client.
//   3. query_api_response::<T>(path, client).await for the typed body,
//      or query_api(...) when we want to inspect the HTTP status code
//      directly (the 404 / 400 cases).

use attestation::{AttestationResponse, AttestorSetResponse, SchemaResponse};
use sov_test_utils::runtime::ApiPath;

/// Path builder that produces `/modules/attestation/<suffix>` matching
/// the routes mounted by `HasCustomRestApi` for the attestation module.
fn attestation_api_path(suffix: &str) -> ApiPath {
    ApiPath::query_module("attestation").with_custom_api_path(suffix)
}

/// Pre-stage state that the query-route tests need: an attestor set, a
/// schema registered against it, and (for the attestation case) one
/// submitted attestation. Returns the ids the tests assert against.
struct StagedQueryState {
    attestor_set_id: attestation::AttestorSetId,
    schema_id: attestation::SchemaId,
    submitter_addr: <S as sov_modules_api::Spec>::Address,
    payload_hash: attestation::PayloadHash,
}

fn stage_query_state(env: &mut TestEnv, signers: &[SigningKey; 3]) -> StagedQueryState {
    let submitter_addr = env.submitter.address();

    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::RegisterAttestorSet { members: safe_members(signers), threshold: 2 },
        ),
        assert: Box::new(|result, _state| assert!(result.tx_receipt.is_successful())),
    });
    let attestor_set_id = AttestorSet::derive_id(&pubkeys_of(signers), 2);

    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::RegisterSchema {
                name: safe_name("themisra.proof-of-prompt"),
                version: 1,
                attestor_set: attestor_set_id,
                fee_routing_bps: 0,
                fee_routing_addr: None,
            },
        ),
        assert: Box::new(|result, _state| assert!(result.tx_receipt.is_successful())),
    });
    let schema_id = Schema::<S>::derive_id(&submitter_addr, "themisra.proof-of-prompt", 1);

    let payload_hash = attestation::PayloadHash::from([0x42u8; 32]);
    let digest = attestation::SignedAttestationPayload::<S> {
        schema_id,
        payload_hash,
        submitter: submitter_addr,
        timestamp: 0,
    }
    .digest();
    let sig_vec: Vec<AttestorSignature> = signers[..2]
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
        SafeVec::<AttestorSignature, MAX_ATTESTATION_SIGNATURES>::try_from(sig_vec).unwrap();
    env.runner.execute_transaction(TransactionTestCase {
        input: env.submitter.create_plain_message::<RT, AttestationModule<S>>(
            CallMessage::SubmitAttestation { schema_id, payload_hash, signatures },
        ),
        assert: Box::new(|result, _state| assert!(result.tx_receipt.is_successful())),
    });

    StagedQueryState { attestor_set_id, schema_id, submitter_addr, payload_hash }
}

#[tokio::test(flavor = "multi_thread")]
async fn query_get_schema_returns_registered_schema() {
    let mut env = setup(Amount::ZERO, Amount::ZERO, Amount::ZERO);
    let signers = sample_signers();
    let staged = stage_query_state(&mut env, &signers);

    let client = env.runner.setup_rest_api_server().await;
    let resp: SchemaResponse<S> = env
        .runner
        .query_api_response(
            &attestation_api_path(&format!("schemas/{}", staged.schema_id)),
            &client,
        )
        .await;

    assert_eq!(resp.schema.owner, staged.submitter_addr);
    assert_eq!(resp.schema.name.as_str(), "themisra.proof-of-prompt");
    assert_eq!(resp.schema.version, 1);
    assert_eq!(resp.schema.attestor_set, staged.attestor_set_id);
}

#[tokio::test(flavor = "multi_thread")]
async fn query_get_attestor_set_returns_registered_set() {
    let mut env = setup(Amount::ZERO, Amount::ZERO, Amount::ZERO);
    let signers = sample_signers();
    let staged = stage_query_state(&mut env, &signers);

    let client = env.runner.setup_rest_api_server().await;
    let resp: AttestorSetResponse = env
        .runner
        .query_api_response(
            &attestation_api_path(&format!("attestor-sets/{}", staged.attestor_set_id)),
            &client,
        )
        .await;

    assert_eq!(resp.attestor_set.threshold, 2);
    // The module sorts members at registration time so the
    // AttestorSetId derivation is deterministic. Compare as sorted
    // sets rather than ordered slices.
    let mut got: Vec<PubKey> = resp.attestor_set.members.as_slice().to_vec();
    got.sort_by_key(|pk| *pk.as_bytes());
    let mut expected = pubkeys_of(&signers);
    expected.sort_by_key(|pk| *pk.as_bytes());
    assert_eq!(got, expected);
}

#[tokio::test(flavor = "multi_thread")]
async fn query_get_attestation_returns_submitted_attestation() {
    let mut env = setup(Amount::ZERO, Amount::ZERO, Amount::ZERO);
    let signers = sample_signers();
    let staged = stage_query_state(&mut env, &signers);

    let attestation_id =
        attestation::AttestationId::from_pair(&staged.schema_id, &staged.payload_hash);

    let client = env.runner.setup_rest_api_server().await;
    let resp: AttestationResponse<S> = env
        .runner
        .query_api_response(
            &attestation_api_path(&format!("attestations/{attestation_id}")),
            &client,
        )
        .await;

    assert_eq!(resp.attestation.schema_id, staged.schema_id);
    assert_eq!(resp.attestation.payload_hash, staged.payload_hash);
    assert_eq!(resp.attestation.submitter, staged.submitter_addr);
    assert_eq!(resp.attestation.signatures.as_slice().len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn query_get_schema_returns_404_for_unknown_id() {
    let mut env = setup(Amount::ZERO, Amount::ZERO, Amount::ZERO);
    let client = env.runner.setup_rest_api_server().await;

    // Well-formed Bech32 with the right HRP, but no schema with this id
    // exists in state. Construct deterministically to avoid relying on
    // a random pick that might collide with a registered schema.
    let unknown_schema_id = attestation::SchemaId::from([0xAAu8; 32]);

    let resp = env
        .runner
        .query_api(
            &attestation_api_path(&format!("schemas/{unknown_schema_id}")),
            &client,
        )
        .await;

    assert_eq!(resp.status().as_u16(), 404);
}

#[tokio::test(flavor = "multi_thread")]
async fn query_get_schema_returns_400_for_invalid_bech32() {
    let mut env = setup(Amount::ZERO, Amount::ZERO, Amount::ZERO);
    let client = env.runner.setup_rest_api_server().await;

    // Garbage path segment. Not a valid Bech32 string.
    let resp = env
        .runner
        .query_api(&attestation_api_path("schemas/not-a-valid-id"), &client)
        .await;

    assert_eq!(resp.status().as_u16(), 400);
}
