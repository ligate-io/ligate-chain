//! Time-gated lifecycle tests for the bounty module (chain#519).
//!
//! These exercise the behaviour that depends on the current chain
//! height, which handlers read via the composed `chain_state` module:
//!
//! - expiry enforcement on `PostBounty` (reject expiry-in-past) and
//!   `ClaimBounty` (reject claims after expiry)
//! - the per-claim `dispute_window_blocks` window on `DisputeAttestation`
//! - the expired-with-claims refund path on `CancelBounty`
//!   (`BountyStatus::Expired`)
//! - `FinaliseBounty` (`BountyStatus::Finalised`), including the
//!   open-dispute and unclosed-window guards
//!
//! Height is advanced with [`TestRunner::advance_slots`]. In the
//! optimistic test genesis `genesis_da_height == 0`, so the chain's
//! "current DA height" equals the rollup height; the tests use generous
//! advance margins so they're insensitive to the exact slot at which a
//! given tx lands.

use attestation::{
    AttestationConfig, AttestationModule, AttestorSet, AttestorSignature, CallMessage as AttCall,
    InitialAttestorSet, InitialSchema, PubKey, Schema, SchemaId, SignedAttestationPayload,
    MAX_ATTESTATION_SIGNATURES, MAX_ATTESTOR_SIGNATURE_BYTES,
};
use bounty::{
    AcceptancePredicate, AttestationClaim, Bounty, BountyId, BountyStatus, CallMessage,
    DisputeGround, MAX_CLAIMS_PER_CALL,
};
use ed25519_dalek::{Signer, SigningKey};
use sov_bank::{config_gas_token_id, Amount, TokenId};
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{SafeVec, TxEffect};
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::TestRunner;
use sov_test_utils::{generate_optimistic_runtime, AsUser, TestUser, TransactionTestCase};

generate_optimistic_runtime!(
    BountyRuntime <=
        attestation: AttestationModule<S>,
        bounty: Bounty<S>
);

type S = sov_test_utils::TestSpec;
type RT = BountyRuntime<S>;

struct TestEnv {
    runner: TestRunner<RT, S>,
    poster: TestUser<S>,
    disputer: TestUser<S>,
    attester_user: TestUser<S>,
    #[allow(dead_code)]
    avow_token_id: TokenId,
    board_schema_id: SchemaId,
    attestor_key: SigningKey,
}

fn single_attestor_signer() -> SigningKey {
    SigningKey::from_bytes(&[42u8; 32])
}

fn setup() -> TestEnv {
    let genesis = HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(3);
    let poster = genesis.additional_accounts()[0].clone();
    let disputer = genesis.additional_accounts()[1].clone();
    let attester_user = genesis.additional_accounts()[2].clone();
    let avow = config_gas_token_id();

    let signer = single_attestor_signer();
    let members = vec![PubKey::from(signer.verifying_key().to_bytes())];
    let initial_attestor_sets = vec![InitialAttestorSet { members: members.clone(), threshold: 1 }];
    let attestor_set_id = AttestorSet::derive_id(&members, 1);
    let schema_name = "themisra.bounty-board.v1".to_string();
    let board_schema_id = Schema::<S>::derive_id(&poster.address(), &schema_name, 1);
    let initial_schemas = vec![InitialSchema::<S> {
        owner: poster.address(),
        name: schema_name,
        version: 1,
        attestor_set: attestor_set_id,
        fee_routing_bps: 0,
        fee_routing_addr: None,
        payload_shape_hash: [0u8; 32],
    }];

    let att_config = AttestationConfig::<S> {
        treasury: poster.address(),
        avow_token_id: avow,
        attestation_fee: Amount::ZERO,
        schema_registration_fee: Amount::ZERO,
        attestor_set_fee: Amount::ZERO,
        initial_attestor_sets,
        initial_schemas,
        max_builder_bps: attestation::DEFAULT_MAX_BUILDER_BPS,
    };
    let bounty_config = bounty::BountyConfig::default();
    let genesis = GenesisConfig::from_minimal_config(genesis.into(), att_config, bounty_config);
    let runner = TestRunner::new_with_genesis(genesis.into_genesis_params(), RT::default());
    TestEnv {
        runner,
        poster,
        disputer,
        attester_user,
        avow_token_id: avow,
        board_schema_id,
        attestor_key: signer,
    }
}

#[track_caller]
fn assert_reverted_with(receipt: &TxEffect<S>, phrase: &str) {
    match receipt {
        TxEffect::Reverted(payload) => {
            let msg = payload.reason.to_string();
            assert!(
                msg.contains(phrase),
                "expected phrase '{phrase}' in reverted reason, got: {msg}"
            );
        }
        other => panic!("expected Reverted, got {other:?}"),
    }
}

/// Build a `PostBounty` with explicit expiry + window so individual
/// tests can drive the time-gated paths.
fn post_with(
    board: SchemaId,
    pool: u128,
    per: u128,
    expiry_da_height: u64,
    dispute_window_blocks: u32,
) -> CallMessage<S> {
    CallMessage::PostBounty {
        board_schema_id: board,
        pool: Amount::new(pool),
        per_attestation: Amount::new(per),
        acceptance: AcceptancePredicate::Any,
        expiry_da_height,
        dispute_window_blocks,
    }
}

fn submit_attestation_via_runner(
    env: &mut TestEnv,
    payload_hash: [u8; 32],
) -> attestation::AttestationId {
    let payload_hash = attestation::PayloadHash::from(payload_hash);
    let submitter = env.attester_user.address();
    let signed = SignedAttestationPayload::<S> {
        schema_id: env.board_schema_id,
        payload_hash,
        submitter,
        timestamp: 0,
    };
    let digest = signed.digest();
    let sig_bytes = env.attestor_key.sign(&digest).to_bytes().to_vec();
    let signatures = SafeVec::<AttestorSignature, MAX_ATTESTATION_SIGNATURES>::try_from(vec![
        AttestorSignature {
            pubkey: PubKey::from(env.attestor_key.verifying_key().to_bytes()),
            sig: SafeVec::<u8, MAX_ATTESTOR_SIGNATURE_BYTES>::try_from(sig_bytes)
                .expect("64 bytes fits MAX_ATTESTOR_SIGNATURE_BYTES"),
        },
    ])
    .expect("1 signature fits MAX_ATTESTATION_SIGNATURES");

    env.runner.execute_transaction(TransactionTestCase {
        input: env.attester_user.create_plain_message::<RT, AttestationModule<S>>(
            AttCall::SubmitAttestation { schema_id: env.board_schema_id, payload_hash, signatures },
        ),
        assert: Box::new(|result, _state| {
            assert!(result.tx_receipt.is_successful(), "SubmitAttestation should succeed in setup");
        }),
    });
    attestation::AttestationId::from_pair(&env.board_schema_id, &payload_hash)
}

fn single_claim(
    attestation_id: attestation::AttestationId,
) -> SafeVec<AttestationClaim, MAX_CLAIMS_PER_CALL> {
    SafeVec::try_from(vec![AttestationClaim { attestation_id }]).expect("1 claim fits")
}

// ----- Expiry on PostBounty ---------------------------------------------------

#[test]
fn post_rejects_expiry_in_past() {
    let mut env = setup();
    let board = env.board_schema_id;
    // Advance well past the small expiry we're about to submit.
    env.runner.advance_slots(50);
    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Bounty<S>>(post_with(board, 500, 100, 10, 50)),
        assert: Box::new(|result, _| {
            assert_reverted_with(&result.tx_receipt, "must be in the future");
        }),
    });
}

// ----- Expiry on ClaimBounty --------------------------------------------------

#[test]
fn claim_rejected_after_expiry() {
    let mut env = setup();
    let board = env.board_schema_id;
    let poster_addr = env.poster.address();

    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Bounty<S>>(post_with(board, 500, 100, 30, 50)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let attestation_id = submit_attestation_via_runner(&mut env, [0xA1u8; 32]);
    let bounty_id = BountyId::derive(poster_addr.as_ref(), &board, 0);

    // Move past the bounty's expiry.
    env.runner.advance_slots(40);

    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Bounty<S>>(CallMessage::ClaimBounty {
            bounty_id,
            claims: single_claim(attestation_id),
        }),
        assert: Box::new(|result, _| {
            assert_reverted_with(&result.tx_receipt, "bounty expired");
        }),
    });
}

// ----- Cancel after expiry with claims ----------------------------------------

#[test]
fn cancel_after_expiry_with_claims_marks_expired_and_refunds() {
    let mut env = setup();
    let board = env.board_schema_id;
    let poster_addr = env.poster.address();

    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Bounty<S>>(post_with(board, 500, 100, 30, 50)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let attestation_id = submit_attestation_via_runner(&mut env, [0xA2u8; 32]);
    let bounty_id = BountyId::derive(poster_addr.as_ref(), &board, 0);

    // One claim is paid (escrow drops below pool), so the pre-expiry
    // cancel rule would reject; expiry unlocks the refund path.
    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Bounty<S>>(CallMessage::ClaimBounty {
            bounty_id,
            claims: single_claim(attestation_id),
        }),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });

    env.runner.advance_slots(40);

    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Bounty<S>>(CallMessage::CancelBounty { bounty_id }),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful(), "expired cancel should succeed");
            let module = Bounty::<S>::default();
            let bounty =
                module.bounties.get(&bounty_id, state).unwrap_infallible().expect("bounty");
            assert_eq!(bounty.status, BountyStatus::Expired);
            assert_eq!(
                module.escrow.get(&bounty_id, state).unwrap_infallible(),
                Some(Amount::ZERO)
            );
        }),
    });
}

// ----- Dispute window ---------------------------------------------------------

#[test]
fn dispute_within_window_succeeds() {
    let mut env = setup();
    let board = env.board_schema_id;
    let poster_addr = env.poster.address();

    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Bounty<S>>(post_with(board, 500, 100, 100_000, 10)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let attestation_id = submit_attestation_via_runner(&mut env, [0xA3u8; 32]);
    let bounty_id = BountyId::derive(poster_addr.as_ref(), &board, 0);
    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Bounty<S>>(CallMessage::ClaimBounty {
            bounty_id,
            claims: single_claim(attestation_id),
        }),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });

    // Dispute immediately, comfortably within the 10-block window.
    env.runner.execute_transaction(TransactionTestCase {
        input: env.disputer.create_plain_message::<RT, Bounty<S>>(
            CallMessage::DisputeAttestation {
                bounty_id,
                attestation_id,
                ground: DisputeGround::InvalidPayload,
            },
        ),
        assert: Box::new(|result, _| {
            assert!(result.tx_receipt.is_successful(), "in-window dispute should succeed");
        }),
    });
}

#[test]
fn dispute_rejected_after_window_closes() {
    let mut env = setup();
    let board = env.board_schema_id;
    let poster_addr = env.poster.address();

    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Bounty<S>>(post_with(board, 500, 100, 100_000, 3)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let attestation_id = submit_attestation_via_runner(&mut env, [0xA4u8; 32]);
    let bounty_id = BountyId::derive(poster_addr.as_ref(), &board, 0);
    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Bounty<S>>(CallMessage::ClaimBounty {
            bounty_id,
            claims: single_claim(attestation_id),
        }),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });

    // Past the 3-block window.
    env.runner.advance_slots(10);

    env.runner.execute_transaction(TransactionTestCase {
        input: env.disputer.create_plain_message::<RT, Bounty<S>>(
            CallMessage::DisputeAttestation {
                bounty_id,
                attestation_id,
                ground: DisputeGround::InvalidPayload,
            },
        ),
        assert: Box::new(|result, _| {
            assert_reverted_with(&result.tx_receipt, "dispute window closed");
        }),
    });
}

#[test]
fn dispute_rejected_without_a_claim() {
    let mut env = setup();
    let board = env.board_schema_id;
    let poster_addr = env.poster.address();

    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Bounty<S>>(post_with(board, 500, 100, 100_000, 50)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    // Attestation exists but was never claimed against the bounty.
    let attestation_id = submit_attestation_via_runner(&mut env, [0xA5u8; 32]);
    let bounty_id = BountyId::derive(poster_addr.as_ref(), &board, 0);

    env.runner.execute_transaction(TransactionTestCase {
        input: env.disputer.create_plain_message::<RT, Bounty<S>>(
            CallMessage::DisputeAttestation {
                bounty_id,
                attestation_id,
                ground: DisputeGround::InvalidPayload,
            },
        ),
        assert: Box::new(|result, _| {
            assert_reverted_with(&result.tx_receipt, "nothing to dispute");
        }),
    });
}

// ----- FinaliseBounty ---------------------------------------------------------

#[test]
fn finalise_after_window_marks_finalised_and_sweeps_dust() {
    let mut env = setup();
    let board = env.board_schema_id;
    let poster_addr = env.poster.address();

    // pool 450, per 100 -> after 1 claim escrow 350 (dust at finalise).
    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Bounty<S>>(post_with(board, 450, 100, 30, 5)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let attestation_id = submit_attestation_via_runner(&mut env, [0xA6u8; 32]);
    let bounty_id = BountyId::derive(poster_addr.as_ref(), &board, 0);
    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Bounty<S>>(CallMessage::ClaimBounty {
            bounty_id,
            claims: single_claim(attestation_id),
        }),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });

    // Past both expiry (30) and the claim's dispute window (5).
    env.runner.advance_slots(40);

    // Permissionless: the disputer (not the poster) finalises.
    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .disputer
            .create_plain_message::<RT, Bounty<S>>(CallMessage::FinaliseBounty { bounty_id }),
        assert: Box::new(move |result, state| {
            assert!(
                result.tx_receipt.is_successful(),
                "finalise should succeed after windows close"
            );
            let module = Bounty::<S>::default();
            let bounty =
                module.bounties.get(&bounty_id, state).unwrap_infallible().expect("bounty");
            assert_eq!(bounty.status, BountyStatus::Finalised);
            assert_eq!(
                module.escrow.get(&bounty_id, state).unwrap_infallible(),
                Some(Amount::ZERO)
            );
        }),
    });
}

#[test]
fn finalise_rejected_with_open_dispute() {
    let mut env = setup();
    let board = env.board_schema_id;
    let poster_addr = env.poster.address();

    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Bounty<S>>(post_with(board, 500, 100, 30, 10)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let attestation_id = submit_attestation_via_runner(&mut env, [0xA7u8; 32]);
    let bounty_id = BountyId::derive(poster_addr.as_ref(), &board, 0);
    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Bounty<S>>(CallMessage::ClaimBounty {
            bounty_id,
            claims: single_claim(attestation_id),
        }),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    // Open a dispute within the window.
    env.runner.execute_transaction(TransactionTestCase {
        input: env.disputer.create_plain_message::<RT, Bounty<S>>(
            CallMessage::DisputeAttestation {
                bounty_id,
                attestation_id,
                ground: DisputeGround::Other,
            },
        ),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });

    // Past expiry, but the dispute is still open.
    env.runner.advance_slots(40);

    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Bounty<S>>(CallMessage::FinaliseBounty { bounty_id }),
        assert: Box::new(|result, _| {
            assert_reverted_with(&result.tx_receipt, "dispute(s) still open");
        }),
    });
}

#[test]
fn finalise_rejected_before_window_closes() {
    let mut env = setup();
    let board = env.board_schema_id;
    let poster_addr = env.poster.address();

    // Large dispute window so it stays open even after expiry passes.
    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Bounty<S>>(post_with(board, 450, 100, 30, 100)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let attestation_id = submit_attestation_via_runner(&mut env, [0xA8u8; 32]);
    let bounty_id = BountyId::derive(poster_addr.as_ref(), &board, 0);
    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Bounty<S>>(CallMessage::ClaimBounty {
            bounty_id,
            claims: single_claim(attestation_id),
        }),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });

    // Past expiry (so the bounty is finalisable) but the claim's
    // 100-block dispute window is still open.
    env.runner.advance_slots(40);

    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Bounty<S>>(CallMessage::FinaliseBounty { bounty_id }),
        assert: Box::new(|result, _| {
            assert_reverted_with(&result.tx_receipt, "dispute windows still open");
        }),
    });
}

#[test]
fn finalise_rejected_when_open_and_not_expired() {
    let mut env = setup();
    let board = env.board_schema_id;
    let poster_addr = env.poster.address();

    // Far-future expiry, no claims: the bounty is plain Open, neither
    // exhausted nor expired, so it cannot be finalised.
    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Bounty<S>>(post_with(board, 500, 100, 100_000, 5)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let bounty_id = BountyId::derive(poster_addr.as_ref(), &board, 0);

    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Bounty<S>>(CallMessage::FinaliseBounty { bounty_id }),
        assert: Box::new(|result, _| {
            assert_reverted_with(&result.tx_receipt, "cannot be finalised");
        }),
    });
}
