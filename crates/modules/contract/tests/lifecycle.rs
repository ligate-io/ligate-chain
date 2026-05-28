//! Time-gated lifecycle tests for the contract module (chain#536).
//!
//! These exercise the behaviour that depends on the current chain
//! height, which handlers read via the composed `chain_state` module:
//!
//! - expiry enforcement on `PostContract` (reject expiry-in-past) and
//!   `DeliverContract` (reject a delivery that misses the window)
//! - `FinalizeDelivery`: the permissionless auto-accept once the
//!   poster's acceptance window has elapsed (the work-for-hire
//!   guarantee), plus the in-window rejection
//! - the expired-cancel refund path on `CancelContract`
//!   (`ContractStatus::Expired`)
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
use contract::{CallMessage, ContractId, ContractStatus, Contracts};
use ed25519_dalek::{Signer, SigningKey};
use sov_bank::{config_gas_token_id, Amount, Bank, TokenId};
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{SafeVec, TxEffect};
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::TestRunner;
use sov_test_utils::{generate_optimistic_runtime, AsUser, TestUser, TransactionTestCase};

generate_optimistic_runtime!(
    ContractRuntime <=
        attestation: AttestationModule<S>,
        contracts: Contracts<S>
);

type S = sov_test_utils::TestSpec;
type RT = ContractRuntime<S>;

const CRITERIA_DOC_HASH: [u8; 32] = [0x7Au8; 32];
const COMMIT_HASH: [u8; 32] = [0x7Bu8; 32];

struct TestEnv {
    runner: TestRunner<RT, S>,
    poster: TestUser<S>,
    worker: TestUser<S>,
    arbiter: TestUser<S>,
    avow_token_id: TokenId,
    /// Schema the deliverable attestation is registered against.
    board_schema_id: SchemaId,
    attestor_key: SigningKey,
}

fn single_attestor_signer() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}

fn setup() -> TestEnv {
    let genesis = HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(3);
    let poster = genesis.additional_accounts()[0].clone();
    let worker = genesis.additional_accounts()[1].clone();
    let arbiter = genesis.additional_accounts()[2].clone();
    let avow = config_gas_token_id();

    // A schema + 1-of-1 attestor set so the worker can publish a
    // deliverable attestation that `DeliverContract` references.
    let signer = single_attestor_signer();
    let members = vec![PubKey::from(signer.verifying_key().to_bytes())];
    let initial_attestor_sets = vec![InitialAttestorSet { members: members.clone(), threshold: 1 }];
    let attestor_set_id = AttestorSet::derive_id(&members, 1);
    let schema_name = "ligate.deliverable.v1".to_string();
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
    let contract_config = contract::ContractConfig::default();
    let genesis = GenesisConfig::from_minimal_config(genesis.into(), att_config, contract_config);
    let runner = TestRunner::new_with_genesis(genesis.into_genesis_params(), RT::default());
    TestEnv {
        runner,
        poster,
        worker,
        arbiter,
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

fn post_with(
    arbiter: <S as sov_modules_api::Spec>::Address,
    pool: u128,
    expiry_da_height: u64,
    dispute_window_blocks: u32,
) -> CallMessage<S> {
    CallMessage::PostContract {
        arbiter,
        criteria_doc_hash: CRITERIA_DOC_HASH,
        pool: Amount::new(pool),
        expiry_da_height,
        dispute_window_blocks,
        arbiter_fee_bps: 500,
    }
}

/// Worker publishes a deliverable attestation; returns its id so
/// `DeliverContract` can reference it.
fn submit_deliverable(env: &mut TestEnv, payload_hash: [u8; 32]) -> attestation::AttestationId {
    let payload_hash = attestation::PayloadHash::from(payload_hash);
    let submitter = env.worker.address();
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
        input: env.worker.create_plain_message::<RT, AttestationModule<S>>(
            AttCall::SubmitAttestation { schema_id: env.board_schema_id, payload_hash, signatures },
        ),
        assert: Box::new(|result, _state| {
            assert!(result.tx_receipt.is_successful(), "SubmitAttestation should succeed in setup");
        }),
    });
    attestation::AttestationId::from_pair(&env.board_schema_id, &payload_hash)
}

/// Drive a contract through post + commit + deliver. Returns the
/// contract id. `expiry` / `window` parameterise the time-gating.
fn post_commit_deliver(
    env: &mut TestEnv,
    expiry: u64,
    window: u32,
    payload: [u8; 32],
) -> ContractId {
    let arbiter_addr = env.arbiter.address();
    let poster_addr = env.poster.address();
    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Contracts<S>>(post_with(
            arbiter_addr,
            1_000,
            expiry,
            window,
        )),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful(), "post should succeed")),
    });
    let contract_id = ContractId::derive(poster_addr.as_ref(), &CRITERIA_DOC_HASH, 0);

    env.runner.execute_transaction(TransactionTestCase {
        input: env.worker.create_plain_message::<RT, Contracts<S>>(CallMessage::CommitToContract {
            contract_id,
            commit_hash: COMMIT_HASH,
            bond: Amount::new(100),
        }),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful(), "commit should succeed")),
    });

    let attestation_id = submit_deliverable(env, payload);
    env.runner.execute_transaction(TransactionTestCase {
        input: env.worker.create_plain_message::<RT, Contracts<S>>(CallMessage::DeliverContract {
            contract_id,
            deliverable_attestation_id: attestation_id,
        }),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful(), "deliver should succeed")),
    });
    contract_id
}

// ----- Expiry on PostContract -------------------------------------------------

#[test]
fn post_rejects_expiry_in_past() {
    let mut env = setup();
    let arbiter_addr = env.arbiter.address();
    env.runner.advance_slots(50);
    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Contracts<S>>(post_with(
            arbiter_addr,
            1_000,
            10,
            5,
        )),
        assert: Box::new(|result, _| {
            assert_reverted_with(&result.tx_receipt, "must be in the future");
        }),
    });
}

// ----- Expiry on DeliverContract ----------------------------------------------

#[test]
fn deliver_rejected_after_expiry() {
    let mut env = setup();
    let arbiter_addr = env.arbiter.address();
    let poster_addr = env.poster.address();

    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Contracts<S>>(post_with(
            arbiter_addr,
            1_000,
            30,
            5,
        )),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let contract_id = ContractId::derive(poster_addr.as_ref(), &CRITERIA_DOC_HASH, 0);
    env.runner.execute_transaction(TransactionTestCase {
        input: env.worker.create_plain_message::<RT, Contracts<S>>(CallMessage::CommitToContract {
            contract_id,
            commit_hash: COMMIT_HASH,
            bond: Amount::new(100),
        }),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let attestation_id = submit_deliverable(&mut env, [0xB1u8; 32]);

    // Past expiry: the delivery misses the window.
    env.runner.advance_slots(40);

    env.runner.execute_transaction(TransactionTestCase {
        input: env.worker.create_plain_message::<RT, Contracts<S>>(CallMessage::DeliverContract {
            contract_id,
            deliverable_attestation_id: attestation_id,
        }),
        assert: Box::new(|result, _| {
            assert_reverted_with(&result.tx_receipt, "contract expired");
        }),
    });
}

// ----- FinalizeDelivery (auto-accept) -----------------------------------------

#[test]
fn finalize_delivery_auto_accepts_after_window() {
    let mut env = setup();
    let avow = env.avow_token_id;
    let worker_addr = env.worker.address();
    let contract_id = post_commit_deliver(&mut env, 100_000, 5, [0xB2u8; 32]);

    // Past the 5-block acceptance window.
    env.runner.advance_slots(20);

    // Permissionless: the worker triggers their own payout.
    env.runner.execute_transaction(TransactionTestCase {
        input: env.worker.create_plain_message::<RT, Contracts<S>>(CallMessage::FinalizeDelivery {
            contract_id,
        }),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful(), "auto-accept should succeed past window");
            let module = Contracts::<S>::default();
            let contract =
                module.contracts.get(&contract_id, state).unwrap_infallible().expect("contract");
            assert_eq!(contract.status, ContractStatus::Accepted);
            assert_eq!(
                module.escrow.get(&contract_id, state).unwrap_infallible(),
                Some(Amount::ZERO)
            );
            // Worker received pool (1000) + bond refund (100).
            let bank = Bank::<S>::default();
            let bal = bank
                .get_balance_of(&worker_addr, avow, state)
                .unwrap_infallible()
                .unwrap_or(Amount::ZERO);
            assert!(bal > Amount::ZERO, "worker paid out, got {bal:?}");
        }),
    });
}

#[test]
fn finalize_delivery_rejected_within_window() {
    let mut env = setup();
    let contract_id = post_commit_deliver(&mut env, 100_000, 50, [0xB3u8; 32]);

    // No advance: the 50-block window is still wide open.
    env.runner.execute_transaction(TransactionTestCase {
        input: env.worker.create_plain_message::<RT, Contracts<S>>(CallMessage::FinalizeDelivery {
            contract_id,
        }),
        assert: Box::new(|result, _| {
            assert_reverted_with(&result.tx_receipt, "acceptance window still open");
        }),
    });
}

// ----- Cancel after expiry ----------------------------------------------------

#[test]
fn cancel_after_expiry_marks_expired_and_refunds() {
    let mut env = setup();
    let arbiter_addr = env.arbiter.address();
    let poster_addr = env.poster.address();

    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Contracts<S>>(post_with(
            arbiter_addr,
            1_000,
            30,
            5,
        )),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let contract_id = ContractId::derive(poster_addr.as_ref(), &CRITERIA_DOC_HASH, 0);
    // A commit moves the contract out of `Open`, so a pre-expiry cancel
    // would be rejected; expiry unlocks the refund path.
    env.runner.execute_transaction(TransactionTestCase {
        input: env.worker.create_plain_message::<RT, Contracts<S>>(CallMessage::CommitToContract {
            contract_id,
            commit_hash: COMMIT_HASH,
            bond: Amount::new(100),
        }),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });

    env.runner.advance_slots(40);

    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Contracts<S>>(CallMessage::CancelContract { contract_id }),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful(), "expired cancel should succeed");
            let module = Contracts::<S>::default();
            let contract =
                module.contracts.get(&contract_id, state).unwrap_infallible().expect("contract");
            assert_eq!(contract.status, ContractStatus::Expired);
            assert_eq!(
                module.escrow.get(&contract_id, state).unwrap_infallible(),
                Some(Amount::ZERO)
            );
        }),
    });
}

#[test]
fn finalize_rejected_when_not_delivered() {
    let mut env = setup();
    let arbiter_addr = env.arbiter.address();
    let poster_addr = env.poster.address();

    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Contracts<S>>(post_with(
            arbiter_addr,
            1_000,
            100_000,
            5,
        )),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let contract_id = ContractId::derive(poster_addr.as_ref(), &CRITERIA_DOC_HASH, 0);
    // Commit but do NOT deliver: status is Committed, not Delivered, so
    // there is nothing to auto-accept.
    env.runner.execute_transaction(TransactionTestCase {
        input: env.worker.create_plain_message::<RT, Contracts<S>>(CallMessage::CommitToContract {
            contract_id,
            commit_hash: COMMIT_HASH,
            bond: Amount::new(100),
        }),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });

    env.runner.execute_transaction(TransactionTestCase {
        input: env.worker.create_plain_message::<RT, Contracts<S>>(CallMessage::FinalizeDelivery {
            contract_id,
        }),
        assert: Box::new(|result, _| {
            assert_reverted_with(&result.tx_receipt, "not valid for this operation");
        }),
    });
}
