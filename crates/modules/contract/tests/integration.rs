//! Unit tests for the contract module's v0 skeleton.
//!
//! These run without the `librocksdb-sys` / `sov-state` transitive
//! deps. Real handler tests via `sov_test_utils::TestRunner` ship
//! alongside the handler implementation PR.

use std::str::FromStr;

use attestation::AttestationId;
use contract::{CommitKey, ContractId};

#[test]
fn contract_id_derive_is_deterministic() {
    let poster = b"lid1examplecontractposteraddrxxxxxxx";
    let criteria_doc_hash = [0x42u8; 32];
    let nonce = 7u64;
    assert_eq!(
        ContractId::derive(poster, &criteria_doc_hash, nonce),
        ContractId::derive(poster, &criteria_doc_hash, nonce)
    );
}

#[test]
fn contract_id_derive_distinguishes_nonces() {
    let poster = b"lid1examplecontractposteraddrxxxxxxx";
    let criteria_doc_hash = [0x42u8; 32];
    assert_ne!(
        ContractId::derive(poster, &criteria_doc_hash, 0),
        ContractId::derive(poster, &criteria_doc_hash, 1)
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
    assert!(s.starts_with("lct1"), "CommitKey Display starts with contract Bech32 prefix");
    assert!(s.contains(':'), "CommitKey Display uses ':' separator");
    let parsed = CommitKey::from_str(&s).expect("FromStr round-trip succeeds");
    assert_eq!(parsed.contract_id, key.contract_id);
    assert_eq!(parsed.worker_bytes, key.worker_bytes);
}

#[test]
fn commit_key_from_str_rejects_malformed() {
    assert!(CommitKey::from_str("").is_err());
    assert!(CommitKey::from_str("no_separator").is_err());
    assert!(CommitKey::from_str("xxx:not_hex").is_err());
    // Wrong-length worker hex (should be exactly 56 hex chars = 28 bytes)
    assert!(CommitKey::from_str("lct1abc:deadbeef").is_err());
}

#[test]
fn attestation_id_unused_warning_silenced() {
    // AttestationId is referenced in the public CallMessage / Event
    // surfaces but not exercised by these unit tests. Touch it here
    // so the dead-import lint doesn't fire.
    let _ = AttestationId::from([0u8; 32]);
}
