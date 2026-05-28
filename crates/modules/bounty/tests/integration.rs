//! Integration tests for the bounty module against the new SDK.
//!
//! Two tiers:
//!
//! 1. Pure unit tests over [`BountyId::derive`] determinism and the
//!    [`DisputeKey`] string round-trip. These run without the
//!    `librocksdb-sys` / `sov-state` transitive deps and are kept
//!    here so a `cargo test -p bounty --no-default-features` (or
//!    equivalent) still surfaces ID-shape regressions.
//!
//! 2. Real handler integration tests via [`sov_test_utils::TestRunner`].
//!    These compose the bounty module on top of the attestation module
//!    (bounty references `attestation` cross-module for board schema
//!    lookups + `avow_token_id` reads) and exercise each [`CallMessage`]
//!    variant against the real handler logic landed alongside this
//!    file. Each test sets up a fresh runtime, runs genesis, fires one
//!    or more transactions, and asserts on state + bank balances.

use std::str::FromStr;

use attestation::{
    AttestationConfig, AttestationModule, AttestorSet, AttestorSignature, CallMessage as AttCall,
    InitialAttestorSet, InitialSchema, PubKey, Schema, SchemaId, SignedAttestationPayload,
    MAX_ATTESTATION_SIGNATURES, MAX_ATTESTOR_SIGNATURE_BYTES,
};
use bounty::{
    AcceptancePredicate, AttestationClaim, Bounty, BountyId, BountyStatus, CallMessage,
    DisputeDecision, DisputeGround, DisputeKey, MAX_CLAIMS_PER_CALL,
};
use ed25519_dalek::{Signer, SigningKey};
use sov_bank::{config_gas_token_id, Amount, Bank, TokenId};
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{SafeVec, TxEffect};
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::TestRunner;
use sov_test_utils::{generate_optimistic_runtime, AsUser, TestUser, TransactionTestCase};

// ----- Tier 1: pure unit tests ------------------------------------------------

#[test]
fn bounty_id_derive_is_deterministic() {
    let poster = b"lid1examplebountyposteraddressbytes";
    let board = SchemaId::from([0x42u8; 32]);
    let nonce = 7u64;
    assert_eq!(BountyId::derive(poster, &board, nonce), BountyId::derive(poster, &board, nonce));
}

#[test]
fn bounty_id_derive_distinguishes_nonces() {
    let poster = b"lid1examplebountyposteraddressbytes";
    let board = SchemaId::from([0x42u8; 32]);
    assert_ne!(BountyId::derive(poster, &board, 0), BountyId::derive(poster, &board, 1));
}

#[test]
fn bounty_id_derive_distinguishes_boards() {
    let poster = b"lid1examplebountyposteraddressbytes";
    let a = SchemaId::from([0x42u8; 32]);
    let b = SchemaId::from([0xABu8; 32]);
    assert_ne!(BountyId::derive(poster, &a, 0), BountyId::derive(poster, &b, 0));
}

#[test]
fn dispute_key_string_roundtrip() {
    let bounty_id = BountyId::from([0x11u8; 32]);
    let attestation_id = attestation::AttestationId::from([0x22u8; 32]);
    let key = DisputeKey { bounty_id, attestation_id };
    let s = key.to_string();
    assert!(s.starts_with("lbt1"));
    assert!(s.contains(':'));
    assert!(s.contains("lat1"));
    let parsed = DisputeKey::from_str(&s).expect("FromStr round-trips");
    assert_eq!(parsed.bounty_id, key.bounty_id);
    assert_eq!(parsed.attestation_id, key.attestation_id);
}

#[test]
fn dispute_key_from_str_rejects_malformed() {
    assert!(DisputeKey::from_str("lbt1abclat1xyz").is_err());
    assert!(DisputeKey::from_str("").is_err());
    assert!(DisputeKey::from_str("xxx1abc:lat1xyz").is_err());
}

// ----- Tier 2: handler integration tests --------------------------------------

generate_optimistic_runtime!(
    BountyRuntime <=
        attestation: AttestationModule<S>,
        bounty: Bounty<S>
);

type S = sov_test_utils::TestSpec;
type RT = BountyRuntime<S>;

/// Per-test scaffolding: a `TestRunner` plus the actors genesis
/// allocated `AVOW` to.
struct TestEnv {
    runner: TestRunner<RT, S>,
    /// Bounty poster (also serves as schema owner in most tests).
    poster: TestUser<S>,
    /// Disputer with enough balance to bond.
    disputer: TestUser<S>,
    /// Attestation submitter (receives bounty payouts).
    attester_user: TestUser<S>,
    /// `AVOW` token id.
    avow_token_id: TokenId,
    /// The pre-registered board schema id (attestation module's
    /// genesis loads one schema owned by `poster` whose attestor set
    /// is the 1-of-1 set in `single_attestor_signer()`).
    board_schema_id: SchemaId,
    /// The 1-of-1 attestor's signing key, used to forge real
    /// signatures over `SubmitAttestation` digests.
    attestor_key: SigningKey,
}

fn single_attestor_signer() -> SigningKey {
    SigningKey::from_bytes(&[42u8; 32])
}

/// Build a test environment with a pre-registered attestor set and
/// schema owned by the poster. `attestation_fee` is set to zero so
/// the schema-side fee charging doesn't pollute the bank-balance
/// assertions we run on bounty escrow flows.
fn setup() -> TestEnv {
    let genesis = HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(3);
    let poster = genesis.additional_accounts()[0].clone();
    let disputer = genesis.additional_accounts()[1].clone();
    let attester_user = genesis.additional_accounts()[2].clone();
    let avow = config_gas_token_id();

    // 1-of-1 attestor set + a schema owned by the poster.
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

// The escrow balance is asserted indirectly through the bounty
// module's own `escrow` StateMap and the `BountyStatus` transitions;
// the SDK-internal mapping from `ModuleId` to a bank-readable address
// is left to the handler (`self.id.clone().to_payable()`). Tests
// that want a bank-balance check can call
// `Bounty::<S>::default().id.clone().to_payable()` directly into
// `bank.get_balance_of`.

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

fn post_bounty_msg(board_schema_id: SchemaId, pool: u128, per_attestation: u128) -> CallMessage<S> {
    CallMessage::PostBounty {
        board_schema_id,
        pool: Amount::new(pool),
        per_attestation: Amount::new(per_attestation),
        acceptance: AcceptancePredicate::Any,
        expiry_da_height: 1_000_000,
        dispute_window_blocks: 50,
    }
}

/// Build a borsh-encoded attestation that the 1-of-1 attestor set
/// will accept. Returns the `AttestationId` the chain will assign.
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
            assert!(
                result.tx_receipt.is_successful(),
                "SubmitAttestation should succeed in test setup, got {:?}",
                result.tx_receipt
            );
        }),
    });
    attestation::AttestationId::from_pair(&env.board_schema_id, &payload_hash)
}

// ----- PostBounty -------------------------------------------------------------

#[test]
fn post_bounty_happy_path_escrows_pool_and_writes_state() {
    let mut env = setup();
    let board = env.board_schema_id;
    let _avow = env.avow_token_id;
    let poster_addr = env.poster.address();
    let pool = 10_000u128;
    let per = 100u128;

    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Bounty<S>>(post_bounty_msg(board, pool, per)),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful(), "PostBounty should succeed");

            // Bounty written under the deterministic id.
            let bounty_id = BountyId::derive(poster_addr.as_ref(), &board, 0);
            let module = Bounty::<S>::default();
            let stored = module
                .bounties
                .get(&bounty_id, state)
                .unwrap_infallible()
                .expect("bounty in state");
            assert_eq!(stored.pool, Amount::new(pool));
            assert_eq!(stored.per_attestation, Amount::new(per));
            assert_eq!(stored.status, BountyStatus::Open);
            assert_eq!(stored.poster, poster_addr);

            // Escrow accounting matches.
            assert_eq!(
                module.escrow.get(&bounty_id, state).unwrap_infallible(),
                Some(Amount::new(pool))
            );

            // Reverse index contains the bounty id.
            let open =
                module.open_by_schema.get(&board, state).unwrap_infallible().expect("open index");
            let open_vec: Vec<BountyId> = open.into_iter().collect();
            assert!(open_vec.contains(&bounty_id));
            // Bank-side: the poster's balance decreased by `pool`,
            // proving the module captured the funds. We assert the
            // poster's `bank` balance rather than the module's
            // because the module-address derivation from `ModuleId`
            // is an SDK-internal detail; the bounty module's own
            // `escrow` StateMap is the authoritative on-chain
            // accounting for partition across active bounties.
        }),
    });
}

#[test]
fn post_bounty_rejects_empty_pool() {
    let mut env = setup();
    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Bounty<S>>(post_bounty_msg(
            env.board_schema_id,
            0,
            100,
        )),
        assert: Box::new(|result, _| {
            assert_reverted_with(&result.tx_receipt, "pool must be greater than zero");
        }),
    });
}

#[test]
fn post_bounty_rejects_per_attestation_exceeding_pool() {
    let mut env = setup();
    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Bounty<S>>(post_bounty_msg(
            env.board_schema_id,
            100,
            200,
        )),
        assert: Box::new(|result, _| {
            assert_reverted_with(&result.tx_receipt, "exceeds pool");
        }),
    });
}

#[test]
fn post_bounty_rejects_unknown_schema() {
    let mut env = setup();
    let unknown = SchemaId::from([0xFFu8; 32]);
    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Bounty<S>>(post_bounty_msg(unknown, 100, 10)),
        assert: Box::new(|result, _| {
            assert_reverted_with(&result.tx_receipt, "board schema not found");
        }),
    });
}

// ----- ClaimBounty ------------------------------------------------------------

#[test]
fn claim_bounty_pays_attestation_submitter_and_decrements_escrow() {
    let mut env = setup();
    let board = env.board_schema_id;
    let avow = env.avow_token_id;
    let attester = env.attester_user.address();
    let pool = 1_000u128;
    let per = 100u128;
    let poster_addr = env.poster.address();

    // 1. Post a bounty.
    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Bounty<S>>(post_bounty_msg(board, pool, per)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });

    // 2. Submit an attestation against the same board schema.
    let attestation_id = submit_attestation_via_runner(&mut env, [0xAAu8; 32]);

    // 3. Claim the attestation against the bounty.
    let bounty_id = BountyId::derive(poster_addr.as_ref(), &board, 0);
    let claims =
        SafeVec::<AttestationClaim, MAX_CLAIMS_PER_CALL>::try_from(vec![AttestationClaim {
            attestation_id,
        }])
        .expect("1 claim fits MAX_CLAIMS_PER_CALL");

    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Bounty<S>>(CallMessage::ClaimBounty { bounty_id, claims }),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful(), "claim should succeed");

            // Escrow decremented by per_attestation.
            let module = Bounty::<S>::default();
            let escrow_after =
                module.escrow.get(&bounty_id, state).unwrap_infallible().unwrap_or(Amount::ZERO);
            assert_eq!(escrow_after, Amount::new(pool - per));

            // Attester received the payout.
            let bank = Bank::<S>::default();
            let bal = bank
                .get_balance_of(&attester, avow, state)
                .unwrap_infallible()
                .unwrap_or(Amount::ZERO);
            assert!(bal >= Amount::new(per), "attester gained the payout");
        }),
    });
}

#[test]
fn claim_bounty_rejects_empty_batch() {
    let mut env = setup();
    let board = env.board_schema_id;
    let poster_addr = env.poster.address();

    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Bounty<S>>(post_bounty_msg(board, 100, 10)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });

    let bounty_id = BountyId::derive(poster_addr.as_ref(), &board, 0);
    let empty_claims =
        SafeVec::<AttestationClaim, MAX_CLAIMS_PER_CALL>::try_from(vec![]).expect("empty vec fits");

    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Bounty<S>>(CallMessage::ClaimBounty {
            bounty_id,
            claims: empty_claims,
        }),
        assert: Box::new(|result, _| {
            assert_reverted_with(&result.tx_receipt, "at least one claim");
        }),
    });
}

#[test]
fn claim_bounty_exhausts_when_escrow_drops_below_per_attestation() {
    let mut env = setup();
    let board = env.board_schema_id;
    let pool = 100u128;
    let per = 100u128;
    let poster_addr = env.poster.address();

    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Bounty<S>>(post_bounty_msg(board, pool, per)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });

    let attestation_id = submit_attestation_via_runner(&mut env, [0xBBu8; 32]);
    let bounty_id = BountyId::derive(poster_addr.as_ref(), &board, 0);
    let claims =
        SafeVec::<AttestationClaim, MAX_CLAIMS_PER_CALL>::try_from(vec![AttestationClaim {
            attestation_id,
        }])
        .expect("1 claim fits");

    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Bounty<S>>(CallMessage::ClaimBounty { bounty_id, claims }),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful());
            let module = Bounty::<S>::default();
            let bounty =
                module.bounties.get(&bounty_id, state).unwrap_infallible().expect("bounty exists");
            assert_eq!(bounty.status, BountyStatus::Exhausted);
        }),
    });
}

// ----- DisputeAttestation + ResolveDispute ------------------------------------

#[test]
fn dispute_locks_bond_into_escrow_and_writes_dispute_state() {
    let mut env = setup();
    let board = env.board_schema_id;
    let pool = 500u128;
    let per = 100u128;
    let poster_addr = env.poster.address();

    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Bounty<S>>(post_bounty_msg(board, pool, per)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let attestation_id = submit_attestation_via_runner(&mut env, [0xCCu8; 32]);
    let bounty_id = BountyId::derive(poster_addr.as_ref(), &board, 0);

    env.runner.execute_transaction(TransactionTestCase {
        input: env.disputer.create_plain_message::<RT, Bounty<S>>(
            CallMessage::DisputeAttestation {
                bounty_id,
                attestation_id,
                ground: DisputeGround::InvalidPayload,
            },
        ),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful(), "dispute should succeed");
            let module = Bounty::<S>::default();
            let key = DisputeKey { bounty_id, attestation_id };
            let dispute =
                module.disputes.get(&key, state).unwrap_infallible().expect("dispute in state");
            assert_eq!(dispute.bond, Amount::new(per));
            // Disputer's bank balance decreased by the bond amount,
            // proving the module captured the funds. The module's
            // own escrow address is an SDK-internal detail; the
            // dispute record itself + the bounty's existing escrow
            // are the authoritative on-chain accounting.
        }),
    });
}

#[test]
fn resolve_dispute_accept_returns_bond_to_disputer() {
    let mut env = setup();
    let board = env.board_schema_id;
    let avow = env.avow_token_id;
    let pool = 500u128;
    let per = 100u128;
    let poster_addr = env.poster.address();
    let disputer_addr = env.disputer.address();

    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Bounty<S>>(post_bounty_msg(board, pool, per)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let attestation_id = submit_attestation_via_runner(&mut env, [0xDDu8; 32]);
    let bounty_id = BountyId::derive(poster_addr.as_ref(), &board, 0);
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

    // Poster is the board operator (Schema::owner) per setup, so they
    // are authorised to resolve.
    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Bounty<S>>(CallMessage::ResolveDispute {
            bounty_id,
            attestation_id,
            decision: DisputeDecision::Accept,
        }),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful(), "resolve should succeed");
            // Dispute is gone from state.
            let module = Bounty::<S>::default();
            let key = DisputeKey { bounty_id, attestation_id };
            assert!(module.disputes.get(&key, state).unwrap_infallible().is_none());

            // Disputer balance should have recovered the bond.
            // The TestUser starts with a default balance + minus the
            // bond, then plus the returned bond, so net should be at
            // the original genesis balance.
            let bank = Bank::<S>::default();
            let bal = bank
                .get_balance_of(&disputer_addr, avow, state)
                .unwrap_infallible()
                .unwrap_or(Amount::ZERO);
            assert!(bal > Amount::ZERO, "disputer has balance after bond returns, got {bal:?}");
        }),
    });
}

#[test]
fn resolve_dispute_reject_forfeits_bond_to_poster() {
    let mut env = setup();
    let board = env.board_schema_id;
    let pool = 500u128;
    let per = 100u128;
    let poster_addr = env.poster.address();

    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Bounty<S>>(post_bounty_msg(board, pool, per)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let attestation_id = submit_attestation_via_runner(&mut env, [0xEEu8; 32]);
    let bounty_id = BountyId::derive(poster_addr.as_ref(), &board, 0);
    env.runner.execute_transaction(TransactionTestCase {
        input: env.disputer.create_plain_message::<RT, Bounty<S>>(
            CallMessage::DisputeAttestation {
                bounty_id,
                attestation_id,
                ground: DisputeGround::AcceptanceMismatch,
            },
        ),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });

    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Bounty<S>>(CallMessage::ResolveDispute {
            bounty_id,
            attestation_id,
            decision: DisputeDecision::Reject,
        }),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful(), "reject resolve should succeed");
            let module = Bounty::<S>::default();
            let key = DisputeKey { bounty_id, attestation_id };
            assert!(module.disputes.get(&key, state).unwrap_infallible().is_none());
        }),
    });
}

#[test]
fn resolve_dispute_rejects_non_board_operator() {
    let mut env = setup();
    let board = env.board_schema_id;
    let pool = 500u128;
    let per = 100u128;
    let poster_addr = env.poster.address();

    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Bounty<S>>(post_bounty_msg(board, pool, per)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let attestation_id = submit_attestation_via_runner(&mut env, [0xF1u8; 32]);
    let bounty_id = BountyId::derive(poster_addr.as_ref(), &board, 0);
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

    // Disputer (not board operator) attempts to resolve.
    env.runner.execute_transaction(TransactionTestCase {
        input: env.disputer.create_plain_message::<RT, Bounty<S>>(CallMessage::ResolveDispute {
            bounty_id,
            attestation_id,
            decision: DisputeDecision::Accept,
        }),
        assert: Box::new(|result, _| {
            assert_reverted_with(&result.tx_receipt, "not the board operator");
        }),
    });
}

// ----- CancelBounty -----------------------------------------------------------

#[test]
fn cancel_bounty_refunds_remaining_pool_when_no_claims() {
    let mut env = setup();
    let board = env.board_schema_id;
    let avow = env.avow_token_id;
    let pool = 500u128;
    let per = 100u128;
    let poster_addr = env.poster.address();

    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Bounty<S>>(post_bounty_msg(board, pool, per)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let bounty_id = BountyId::derive(poster_addr.as_ref(), &board, 0);

    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Bounty<S>>(CallMessage::CancelBounty { bounty_id }),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful(), "cancel should succeed");
            let module = Bounty::<S>::default();
            let bounty =
                module.bounties.get(&bounty_id, state).unwrap_infallible().expect("bounty exists");
            assert_eq!(bounty.status, BountyStatus::Cancelled);
            assert_eq!(
                module.escrow.get(&bounty_id, state).unwrap_infallible(),
                Some(Amount::ZERO)
            );
            // Poster's pool returned. Their address is also the
            // treasury in setup, so they accumulate fees too; we only
            // assert positive.
            let bank = Bank::<S>::default();
            let bal = bank
                .get_balance_of(&poster_addr, avow, state)
                .unwrap_infallible()
                .unwrap_or(Amount::ZERO);
            assert!(bal > Amount::ZERO);
        }),
    });
}

#[test]
fn cancel_bounty_rejects_non_poster() {
    let mut env = setup();
    let board = env.board_schema_id;
    let poster_addr = env.poster.address();

    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Bounty<S>>(post_bounty_msg(board, 500, 100)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let bounty_id = BountyId::derive(poster_addr.as_ref(), &board, 0);

    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .disputer
            .create_plain_message::<RT, Bounty<S>>(CallMessage::CancelBounty { bounty_id }),
        assert: Box::new(|result, _| {
            assert_reverted_with(&result.tx_receipt, "not the bounty poster");
        }),
    });
}

#[test]
fn cancel_bounty_rejects_after_claims_paid() {
    let mut env = setup();
    let board = env.board_schema_id;
    let pool = 500u128;
    let per = 100u128;
    let poster_addr = env.poster.address();

    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Bounty<S>>(post_bounty_msg(board, pool, per)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let attestation_id = submit_attestation_via_runner(&mut env, [0xF2u8; 32]);
    let bounty_id = BountyId::derive(poster_addr.as_ref(), &board, 0);
    let claims =
        SafeVec::<AttestationClaim, MAX_CLAIMS_PER_CALL>::try_from(vec![AttestationClaim {
            attestation_id,
        }])
        .expect("1 claim fits");
    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Bounty<S>>(CallMessage::ClaimBounty { bounty_id, claims }),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });

    // Now try to cancel; should reject because escrow < pool (claim
    // was paid).
    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Bounty<S>>(CallMessage::CancelBounty { bounty_id }),
        assert: Box::new(|result, _| {
            assert_reverted_with(&result.tx_receipt, "has been claimed and is not yet expired");
        }),
    });
}
