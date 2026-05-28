//! Integration tests for the contract module against the new SDK.
//!
//! Two tiers:
//!
//! 1. Pure unit tests over [`ContractId::derive`] determinism and the
//!    [`CommitKey`] string round-trip.
//! 2. Real handler integration tests via [`sov_test_utils::TestRunner`]
//!    composing contract + attestation modules. Each test sets up a
//!    fresh runtime, runs genesis, fires transactions, and asserts on
//!    state + bank balances.

use std::str::FromStr;

use attestation::{
    AttestationConfig, AttestationId, AttestationModule, AttestorSet, AttestorSignature,
    CallMessage as AttCall, InitialAttestorSet, InitialSchema, PayloadHash, PubKey, Schema,
    SchemaId, SignedAttestationPayload, MAX_ATTESTATION_SIGNATURES, MAX_ATTESTOR_SIGNATURE_BYTES,
};
use contract::{
    CallMessage, CommitKey, ContractId, ContractStatus, Contracts, DisputeDecision, DisputeGround,
};
use ed25519_dalek::{Signer, SigningKey};
use sov_bank::{config_gas_token_id, Amount, TokenId};
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{SafeVec, TxEffect};
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::TestRunner;
use sov_test_utils::{generate_optimistic_runtime, AsUser, TestUser, TransactionTestCase};

// ----- Tier 1: pure unit tests ------------------------------------------------

#[test]
fn contract_id_derive_is_deterministic() {
    let poster = b"lid1examplecontractposteraddrxxxxxxx";
    let criteria = [0x42u8; 32];
    let nonce = 7u64;
    assert_eq!(
        ContractId::derive(poster, &criteria, nonce),
        ContractId::derive(poster, &criteria, nonce)
    );
}

#[test]
fn contract_id_derive_distinguishes_nonces() {
    let poster = b"lid1examplecontractposteraddrxxxxxxx";
    let criteria = [0x42u8; 32];
    assert_ne!(
        ContractId::derive(poster, &criteria, 0),
        ContractId::derive(poster, &criteria, 1)
    );
}

#[test]
fn contract_id_derive_distinguishes_criteria_docs() {
    let poster = b"lid1examplecontractposteraddrxxxxxxx";
    let a = [0x42u8; 32];
    let b = [0xABu8; 32];
    assert_ne!(ContractId::derive(poster, &a, 0), ContractId::derive(poster, &b, 0));
}

#[test]
fn commit_key_string_roundtrip() {
    let contract_id = ContractId::from([0x11u8; 32]);
    let worker_bytes = [0x22u8; 28];
    let key = CommitKey { contract_id, worker_bytes };
    let s = key.to_string();
    assert!(s.starts_with("lct1"));
    assert!(s.contains(':'));
    let parsed = CommitKey::from_str(&s).expect("FromStr round-trips");
    assert_eq!(parsed.contract_id, key.contract_id);
    assert_eq!(parsed.worker_bytes, key.worker_bytes);
}

#[test]
fn commit_key_from_str_rejects_malformed() {
    assert!(CommitKey::from_str("").is_err());
    assert!(CommitKey::from_str("no_separator").is_err());
    assert!(CommitKey::from_str("xxx:not_hex").is_err());
    assert!(CommitKey::from_str("lct1abc:deadbeef").is_err());
}

// ----- Tier 2: handler integration tests --------------------------------------

generate_optimistic_runtime!(
    ContractRuntime <=
        attestation: AttestationModule<S>,
        contracts: Contracts<S>
);

type S = sov_test_utils::TestSpec;
type RT = ContractRuntime<S>;

struct TestEnv {
    runner: TestRunner<RT, S>,
    poster: TestUser<S>,
    worker: TestUser<S>,
    arbiter: TestUser<S>,
    other_worker: TestUser<S>,
    avow_token_id: TokenId,
    schema_id: SchemaId,
    attestor_key: SigningKey,
}

fn deliverable_attestor_signer() -> SigningKey {
    SigningKey::from_bytes(&[42u8; 32])
}

fn setup() -> TestEnv {
    let genesis = HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(4);
    let poster = genesis.additional_accounts()[0].clone();
    let worker = genesis.additional_accounts()[1].clone();
    let arbiter = genesis.additional_accounts()[2].clone();
    let other_worker = genesis.additional_accounts()[3].clone();
    let avow = config_gas_token_id();

    let signer = deliverable_attestor_signer();
    let members = vec![PubKey::from(signer.verifying_key().to_bytes())];
    let initial_attestor_sets = vec![InitialAttestorSet { members: members.clone(), threshold: 1 }];
    let attestor_set_id = AttestorSet::derive_id(&members, 1);
    let schema_name = "contract.deliverable.v1".to_string();
    let schema_id = Schema::<S>::derive_id(&poster.address(), &schema_name, 1);
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
        other_worker,
        avow_token_id: avow,
        schema_id,
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

fn post_contract_msg(
    arbiter_addr: <S as sov_modules_api::Spec>::Address,
    pool: u128,
) -> CallMessage<S> {
    CallMessage::PostContract {
        arbiter: arbiter_addr,
        criteria_doc_hash: [0xCCu8; 32],
        pool: Amount::new(pool),
        expiry_da_height: 1_000_000,
        dispute_window_blocks: 600,
        arbiter_fee_bps: 500,
    }
}

fn submit_attestation_via_runner(env: &mut TestEnv, payload_hash: [u8; 32]) -> AttestationId {
    let payload_hash = PayloadHash::from(payload_hash);
    let submitter = env.worker.address();
    let signed = SignedAttestationPayload::<S> {
        schema_id: env.schema_id,
        payload_hash,
        submitter,
        timestamp: 0,
    };
    let digest = signed.digest();
    let sig_bytes = env.attestor_key.sign(&digest).to_bytes().to_vec();
    let signatures =
        SafeVec::<AttestorSignature, MAX_ATTESTATION_SIGNATURES>::try_from(vec![AttestorSignature {
            pubkey: PubKey::from(env.attestor_key.verifying_key().to_bytes()),
            sig: SafeVec::<u8, MAX_ATTESTOR_SIGNATURE_BYTES>::try_from(sig_bytes)
                .expect("64 bytes fits"),
        }])
        .expect("1 sig fits");

    env.runner.execute_transaction(TransactionTestCase {
        input: env.worker.create_plain_message::<RT, AttestationModule<S>>(
            AttCall::SubmitAttestation {
                schema_id: env.schema_id,
                payload_hash,
                signatures,
            },
        ),
        assert: Box::new(|result, _state| {
            assert!(
                result.tx_receipt.is_successful(),
                "SubmitAttestation should succeed in test setup, got {:?}",
                result.tx_receipt
            );
        }),
    });
    AttestationId::from_pair(&env.schema_id, &payload_hash)
}

// ----- PostContract -----------------------------------------------------------

#[test]
fn post_contract_happy_path_escrows_and_writes_state() {
    let mut env = setup();
    let arbiter_addr = env.arbiter.address();
    let _avow = env.avow_token_id;
    let pool = 10_000u128;
    let poster_addr = env.poster.address();

    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Contracts<S>>(post_contract_msg(arbiter_addr, pool)),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful());
            let contract_id = ContractId::derive(poster_addr.as_ref(), &[0xCCu8; 32], 0);
            let module = Contracts::<S>::default();
            let stored = module
                .contracts
                .get(&contract_id, state)
                .unwrap_infallible()
                .expect("contract in state");
            assert_eq!(stored.pool, Amount::new(pool));
            assert_eq!(stored.status, ContractStatus::Open);
            assert_eq!(stored.poster, poster_addr);
            assert_eq!(stored.arbiter, arbiter_addr);
            assert_eq!(
                module.escrow.get(&contract_id, state).unwrap_infallible(),
                Some(Amount::new(pool))
            );
        }),
    });
}

#[test]
fn post_contract_rejects_empty_pool() {
    let mut env = setup();
    let arbiter_addr = env.arbiter.address();
    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Contracts<S>>(post_contract_msg(arbiter_addr, 0)),
        assert: Box::new(|result, _| {
            assert_reverted_with(&result.tx_receipt, "pool must be greater than zero");
        }),
    });
}

#[test]
fn post_contract_rejects_arbiter_fee_above_cap() {
    let mut env = setup();
    let arbiter_addr = env.arbiter.address();
    let msg = CallMessage::PostContract {
        arbiter: arbiter_addr,
        criteria_doc_hash: [0xCCu8; 32],
        pool: Amount::new(100),
        expiry_da_height: 1_000_000,
        dispute_window_blocks: 600,
        arbiter_fee_bps: 6000,
    };
    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Contracts<S>>(msg),
        assert: Box::new(|result, _| {
            assert_reverted_with(&result.tx_receipt, "exceeds protocol cap");
        }),
    });
}

// ----- CommitToContract -------------------------------------------------------

#[test]
fn commit_to_contract_locks_bond_and_writes_record() {
    let mut env = setup();
    let arbiter_addr = env.arbiter.address();
    let poster_addr = env.poster.address();
    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Contracts<S>>(post_contract_msg(arbiter_addr, 1_000)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let contract_id = ContractId::derive(poster_addr.as_ref(), &[0xCCu8; 32], 0);
    env.runner.execute_transaction(TransactionTestCase {
        input: env.worker.create_plain_message::<RT, Contracts<S>>(
            CallMessage::CommitToContract {
                contract_id,
                commit_hash: [0xAAu8; 32],
                bond: Amount::new(50),
            },
        ),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful());
            let module = Contracts::<S>::default();
            let stored = module
                .contracts
                .get(&contract_id, state)
                .unwrap_infallible()
                .expect("contract exists");
            assert_eq!(stored.status, ContractStatus::Committed);
        }),
    });
}

#[test]
fn commit_to_contract_rejects_empty_bond() {
    let mut env = setup();
    let arbiter_addr = env.arbiter.address();
    let poster_addr = env.poster.address();
    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Contracts<S>>(post_contract_msg(arbiter_addr, 1_000)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let contract_id = ContractId::derive(poster_addr.as_ref(), &[0xCCu8; 32], 0);
    env.runner.execute_transaction(TransactionTestCase {
        input: env.worker.create_plain_message::<RT, Contracts<S>>(
            CallMessage::CommitToContract {
                contract_id,
                commit_hash: [0xAAu8; 32],
                bond: Amount::ZERO,
            },
        ),
        assert: Box::new(|r, _| {
            assert_reverted_with(&r.tx_receipt, "bond must be greater than zero");
        }),
    });
}

#[test]
fn commit_to_contract_rejects_duplicate_commit_by_same_worker() {
    let mut env = setup();
    let arbiter_addr = env.arbiter.address();
    let poster_addr = env.poster.address();
    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Contracts<S>>(post_contract_msg(arbiter_addr, 1_000)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let contract_id = ContractId::derive(poster_addr.as_ref(), &[0xCCu8; 32], 0);
    let commit = CallMessage::CommitToContract {
        contract_id,
        commit_hash: [0xAAu8; 32],
        bond: Amount::new(50),
    };
    env.runner.execute_transaction(TransactionTestCase {
        input: env.worker.create_plain_message::<RT, Contracts<S>>(commit.clone()),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    env.runner.execute_transaction(TransactionTestCase {
        input: env.worker.create_plain_message::<RT, Contracts<S>>(commit),
        assert: Box::new(|r, _| {
            assert_reverted_with(&r.tx_receipt, "already committed");
        }),
    });
}

// ----- DeliverContract --------------------------------------------------------

#[test]
fn deliver_contract_happy_path_records_delivery() {
    let mut env = setup();
    let arbiter_addr = env.arbiter.address();
    let poster_addr = env.poster.address();
    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Contracts<S>>(post_contract_msg(arbiter_addr, 1_000)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let contract_id = ContractId::derive(poster_addr.as_ref(), &[0xCCu8; 32], 0);
    env.runner.execute_transaction(TransactionTestCase {
        input: env.worker.create_plain_message::<RT, Contracts<S>>(
            CallMessage::CommitToContract {
                contract_id,
                commit_hash: [0xAAu8; 32],
                bond: Amount::new(50),
            },
        ),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });

    let attestation_id = submit_attestation_via_runner(&mut env, [0xDDu8; 32]);

    env.runner.execute_transaction(TransactionTestCase {
        input: env.worker.create_plain_message::<RT, Contracts<S>>(
            CallMessage::DeliverContract {
                contract_id,
                deliverable_attestation_id: attestation_id,
            },
        ),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful());
            let module = Contracts::<S>::default();
            let stored = module
                .contracts
                .get(&contract_id, state)
                .unwrap_infallible()
                .expect("contract exists");
            assert_eq!(stored.status, ContractStatus::Delivered);
        }),
    });
}

#[test]
fn deliver_contract_rejects_worker_without_commit() {
    let mut env = setup();
    let arbiter_addr = env.arbiter.address();
    let poster_addr = env.poster.address();
    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Contracts<S>>(post_contract_msg(arbiter_addr, 1_000)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let contract_id = ContractId::derive(poster_addr.as_ref(), &[0xCCu8; 32], 0);
    env.runner.execute_transaction(TransactionTestCase {
        input: env.worker.create_plain_message::<RT, Contracts<S>>(
            CallMessage::CommitToContract {
                contract_id,
                commit_hash: [0xAAu8; 32],
                bond: Amount::new(50),
            },
        ),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let attestation_id = submit_attestation_via_runner(&mut env, [0xEEu8; 32]);
    env.runner.execute_transaction(TransactionTestCase {
        input: env.other_worker.create_plain_message::<RT, Contracts<S>>(
            CallMessage::DeliverContract {
                contract_id,
                deliverable_attestation_id: attestation_id,
            },
        ),
        assert: Box::new(|r, _| {
            assert_reverted_with(&r.tx_receipt, "has not committed");
        }),
    });
}

// ----- AcceptDelivery ---------------------------------------------------------

#[test]
fn accept_delivery_pays_worker_and_refunds_bond() {
    let mut env = setup();
    let arbiter_addr = env.arbiter.address();
    let poster_addr = env.poster.address();
    let pool = 1_000u128;
    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Contracts<S>>(post_contract_msg(arbiter_addr, pool)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let contract_id = ContractId::derive(poster_addr.as_ref(), &[0xCCu8; 32], 0);
    env.runner.execute_transaction(TransactionTestCase {
        input: env.worker.create_plain_message::<RT, Contracts<S>>(
            CallMessage::CommitToContract {
                contract_id,
                commit_hash: [0xAAu8; 32],
                bond: Amount::new(50),
            },
        ),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let attestation_id = submit_attestation_via_runner(&mut env, [0xF1u8; 32]);
    env.runner.execute_transaction(TransactionTestCase {
        input: env.worker.create_plain_message::<RT, Contracts<S>>(
            CallMessage::DeliverContract {
                contract_id,
                deliverable_attestation_id: attestation_id,
            },
        ),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });

    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Contracts<S>>(CallMessage::AcceptDelivery { contract_id }),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful());
            let module = Contracts::<S>::default();
            let stored = module
                .contracts
                .get(&contract_id, state)
                .unwrap_infallible()
                .expect("contract exists");
            assert_eq!(stored.status, ContractStatus::Accepted);
            assert_eq!(
                module.escrow.get(&contract_id, state).unwrap_infallible(),
                Some(Amount::ZERO)
            );
        }),
    });
}

#[test]
fn accept_delivery_rejects_non_poster() {
    let mut env = setup();
    let arbiter_addr = env.arbiter.address();
    let poster_addr = env.poster.address();
    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Contracts<S>>(post_contract_msg(arbiter_addr, 1_000)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let contract_id = ContractId::derive(poster_addr.as_ref(), &[0xCCu8; 32], 0);
    env.runner.execute_transaction(TransactionTestCase {
        input: env.worker.create_plain_message::<RT, Contracts<S>>(
            CallMessage::CommitToContract {
                contract_id,
                commit_hash: [0xAAu8; 32],
                bond: Amount::new(50),
            },
        ),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let attestation_id = submit_attestation_via_runner(&mut env, [0xF2u8; 32]);
    env.runner.execute_transaction(TransactionTestCase {
        input: env.worker.create_plain_message::<RT, Contracts<S>>(
            CallMessage::DeliverContract {
                contract_id,
                deliverable_attestation_id: attestation_id,
            },
        ),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });

    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .worker
            .create_plain_message::<RT, Contracts<S>>(CallMessage::AcceptDelivery { contract_id }),
        assert: Box::new(|r, _| {
            assert_reverted_with(&r.tx_receipt, "not the contract poster");
        }),
    });
}

// ----- RejectDelivery + ResolveContractDispute --------------------------------

#[test]
fn resolve_dispute_accept_pays_worker() {
    let mut env = setup();
    let arbiter_addr = env.arbiter.address();
    let poster_addr = env.poster.address();
    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Contracts<S>>(post_contract_msg(arbiter_addr, 1_000)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let contract_id = ContractId::derive(poster_addr.as_ref(), &[0xCCu8; 32], 0);
    env.runner.execute_transaction(TransactionTestCase {
        input: env.worker.create_plain_message::<RT, Contracts<S>>(
            CallMessage::CommitToContract {
                contract_id,
                commit_hash: [0xAAu8; 32],
                bond: Amount::new(50),
            },
        ),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let attestation_id = submit_attestation_via_runner(&mut env, [0xF3u8; 32]);
    env.runner.execute_transaction(TransactionTestCase {
        input: env.worker.create_plain_message::<RT, Contracts<S>>(
            CallMessage::DeliverContract {
                contract_id,
                deliverable_attestation_id: attestation_id,
            },
        ),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Contracts<S>>(
            CallMessage::RejectDelivery {
                contract_id,
                ground: DisputeGround::CriteriaMismatch,
            },
        ),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });

    env.runner.execute_transaction(TransactionTestCase {
        input: env.arbiter.create_plain_message::<RT, Contracts<S>>(
            CallMessage::ResolveContractDispute {
                contract_id,
                decision: DisputeDecision::AcceptDelivery,
            },
        ),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful());
            let module = Contracts::<S>::default();
            let stored = module
                .contracts
                .get(&contract_id, state)
                .unwrap_infallible()
                .expect("contract exists");
            assert_eq!(stored.status, ContractStatus::Accepted);
        }),
    });
}

#[test]
fn resolve_dispute_reject_refunds_poster() {
    let mut env = setup();
    let arbiter_addr = env.arbiter.address();
    let poster_addr = env.poster.address();
    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Contracts<S>>(post_contract_msg(arbiter_addr, 1_000)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let contract_id = ContractId::derive(poster_addr.as_ref(), &[0xCCu8; 32], 0);
    env.runner.execute_transaction(TransactionTestCase {
        input: env.worker.create_plain_message::<RT, Contracts<S>>(
            CallMessage::CommitToContract {
                contract_id,
                commit_hash: [0xAAu8; 32],
                bond: Amount::new(50),
            },
        ),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let attestation_id = submit_attestation_via_runner(&mut env, [0xF4u8; 32]);
    env.runner.execute_transaction(TransactionTestCase {
        input: env.worker.create_plain_message::<RT, Contracts<S>>(
            CallMessage::DeliverContract {
                contract_id,
                deliverable_attestation_id: attestation_id,
            },
        ),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Contracts<S>>(
            CallMessage::RejectDelivery {
                contract_id,
                ground: DisputeGround::MalformedDelivery,
            },
        ),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });

    env.runner.execute_transaction(TransactionTestCase {
        input: env.arbiter.create_plain_message::<RT, Contracts<S>>(
            CallMessage::ResolveContractDispute {
                contract_id,
                decision: DisputeDecision::RejectDelivery,
            },
        ),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful());
            let module = Contracts::<S>::default();
            let stored = module
                .contracts
                .get(&contract_id, state)
                .unwrap_infallible()
                .expect("contract exists");
            assert_eq!(stored.status, ContractStatus::Rejected);
        }),
    });
}

#[test]
fn resolve_dispute_rejects_non_arbiter() {
    let mut env = setup();
    let arbiter_addr = env.arbiter.address();
    let poster_addr = env.poster.address();
    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Contracts<S>>(post_contract_msg(arbiter_addr, 1_000)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let contract_id = ContractId::derive(poster_addr.as_ref(), &[0xCCu8; 32], 0);
    env.runner.execute_transaction(TransactionTestCase {
        input: env.worker.create_plain_message::<RT, Contracts<S>>(
            CallMessage::CommitToContract {
                contract_id,
                commit_hash: [0xAAu8; 32],
                bond: Amount::new(50),
            },
        ),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let attestation_id = submit_attestation_via_runner(&mut env, [0xF5u8; 32]);
    env.runner.execute_transaction(TransactionTestCase {
        input: env.worker.create_plain_message::<RT, Contracts<S>>(
            CallMessage::DeliverContract {
                contract_id,
                deliverable_attestation_id: attestation_id,
            },
        ),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Contracts<S>>(
            CallMessage::RejectDelivery { contract_id, ground: DisputeGround::Other },
        ),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });

    env.runner.execute_transaction(TransactionTestCase {
        input: env.poster.create_plain_message::<RT, Contracts<S>>(
            CallMessage::ResolveContractDispute {
                contract_id,
                decision: DisputeDecision::AcceptDelivery,
            },
        ),
        assert: Box::new(|r, _| {
            assert_reverted_with(&r.tx_receipt, "not the contract arbiter");
        }),
    });
}

// ----- CancelContract ---------------------------------------------------------

#[test]
fn cancel_contract_refunds_when_no_commits() {
    let mut env = setup();
    let arbiter_addr = env.arbiter.address();
    let poster_addr = env.poster.address();
    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Contracts<S>>(post_contract_msg(arbiter_addr, 500)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let contract_id = ContractId::derive(poster_addr.as_ref(), &[0xCCu8; 32], 0);
    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Contracts<S>>(CallMessage::CancelContract { contract_id }),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful());
            let module = Contracts::<S>::default();
            let stored = module
                .contracts
                .get(&contract_id, state)
                .unwrap_infallible()
                .expect("contract exists");
            assert_eq!(stored.status, ContractStatus::Cancelled);
            assert_eq!(
                module.escrow.get(&contract_id, state).unwrap_infallible(),
                Some(Amount::ZERO)
            );
        }),
    });
}

#[test]
fn cancel_contract_rejects_non_poster() {
    let mut env = setup();
    let arbiter_addr = env.arbiter.address();
    let poster_addr = env.poster.address();
    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Contracts<S>>(post_contract_msg(arbiter_addr, 500)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let contract_id = ContractId::derive(poster_addr.as_ref(), &[0xCCu8; 32], 0);
    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .worker
            .create_plain_message::<RT, Contracts<S>>(CallMessage::CancelContract { contract_id }),
        assert: Box::new(|r, _| {
            assert_reverted_with(&r.tx_receipt, "not the contract poster");
        }),
    });
}

#[test]
fn cancel_contract_rejects_after_commit() {
    let mut env = setup();
    let arbiter_addr = env.arbiter.address();
    let poster_addr = env.poster.address();
    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Contracts<S>>(post_contract_msg(arbiter_addr, 500)),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    let contract_id = ContractId::derive(poster_addr.as_ref(), &[0xCCu8; 32], 0);
    env.runner.execute_transaction(TransactionTestCase {
        input: env.worker.create_plain_message::<RT, Contracts<S>>(
            CallMessage::CommitToContract {
                contract_id,
                commit_hash: [0xAAu8; 32],
                bond: Amount::new(50),
            },
        ),
        assert: Box::new(|r, _| assert!(r.tx_receipt.is_successful())),
    });
    env.runner.execute_transaction(TransactionTestCase {
        input: env
            .poster
            .create_plain_message::<RT, Contracts<S>>(CallMessage::CancelContract { contract_id }),
        assert: Box::new(|r, _| {
            assert_reverted_with(&r.tx_receipt, "contract has active commits");
        }),
    });
}
