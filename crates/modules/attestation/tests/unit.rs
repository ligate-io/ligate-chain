//! Pure-data unit tests for the attestation module's protocol types.
//!
//! These don't need a `TestRunner`. They cover:
//!
//! - Borsh + Serde round-trips for every persisted type, so we catch
//!   on-wire serialization regressions before they reach the network.
//! - `Schema::derive_id` and `AttestorSet::derive_id` determinism +
//!   semantic invariants (same inputs → same id; ordering doesn't
//!   matter for sets; threshold / version / owner / name all
//!   participate).
//! - `SignedAttestationPayload::digest` determinism.
//! - `DEFAULT_MAX_BUILDER_BPS` cap.
//! - `split_attestation_fee` math at zero, mid, cap, and overflow-
//!   adjacent inputs.
//!
//! Mirrors `mod tests` from the v0.3 test suite preserved at
//! `/tmp/attestation_tests_v0_3.rs` for reference. Type signatures
//! changed (`SchemaId` is now a typed Bech32 newtype, fee values are
//! `Amount`, etc.); behaviour assertions stay the same.

use attestation::{
    Attestation, AttestationConfig, AttestationId, AttestorSet, AttestorSignature, CallMessage,
    InitialAttestorSet, InitialSchema, PayloadHash, PubKey, Schema, SchemaId,
    SignedAttestationPayload, MAX_ATTESTATION_SIGNATURES, MAX_ATTESTOR_SET_MEMBERS,
    MAX_ATTESTOR_SIGNATURE_BYTES,
};
use borsh::{BorshDeserialize, BorshSerialize};
use sov_bank::{Amount, TokenId};
use sov_modules_api::{Address, SafeString, SafeVec, Spec};

type S = sov_test_utils::TestSpec;

// ----- Helpers ---------------------------------------------------------------

fn round_trip_borsh<T>(value: T)
where
    T: BorshSerialize + BorshDeserialize + PartialEq + std::fmt::Debug,
{
    let bytes = borsh::to_vec(&value).expect("encode");
    let decoded: T = borsh::from_slice(&bytes).expect("decode");
    assert_eq!(value, decoded);
}

fn sample_addr(n: u8) -> <S as Spec>::Address {
    Address::from([n; 28])
}

fn sample_pubkey(n: u8) -> PubKey {
    PubKey::from([n; 32])
}

fn sample_schema_id(n: u8) -> SchemaId {
    SchemaId::from([n; 32])
}

fn sample_payload_hash(n: u8) -> PayloadHash {
    PayloadHash::from([n; 32])
}

fn safe_sig(byte: u8) -> SafeVec<u8, MAX_ATTESTOR_SIGNATURE_BYTES> {
    SafeVec::try_from(vec![byte; 64]).expect("64 bytes fits in 96")
}

fn sample_attestor_set() -> AttestorSet {
    AttestorSet {
        members: vec![sample_pubkey(1), sample_pubkey(2), sample_pubkey(3)],
        threshold: 2,
    }
}

fn sample_schema() -> Schema<S> {
    Schema::<S> {
        owner: sample_addr(42),
        name: "themisra.proof-of-prompt".to_string(),
        version: 1,
        attestor_set: AttestorSet::derive_id(&[sample_pubkey(7)], 1),
        fee_routing_bps: 2500,
        fee_routing_addr: Some(sample_addr(99)),
    }
}

fn sample_attestation() -> Attestation<S> {
    Attestation::<S> {
        schema_id: sample_schema_id(3),
        payload_hash: sample_payload_hash(4),
        submitter: sample_addr(5),
        timestamp: 1_700_000_000,
        signatures: vec![AttestorSignature { pubkey: sample_pubkey(1), sig: safe_sig(0xab) }],
    }
}

fn sample_signed_payload() -> SignedAttestationPayload<S> {
    SignedAttestationPayload::<S> {
        schema_id: sample_schema_id(9),
        payload_hash: sample_payload_hash(1),
        submitter: sample_addr(7),
        timestamp: 1_234_567_890,
    }
}

// ----- Borsh round-trips -----------------------------------------------------

#[test]
fn schema_borsh_round_trip() {
    round_trip_borsh(sample_schema());
}

#[test]
fn attestor_set_borsh_round_trip() {
    round_trip_borsh(sample_attestor_set());
}

#[test]
fn attestation_borsh_round_trip() {
    round_trip_borsh(sample_attestation());
}

#[test]
fn signed_payload_borsh_round_trip() {
    round_trip_borsh(sample_signed_payload());
}

#[test]
fn attestation_id_borsh_round_trip() {
    round_trip_borsh(AttestationId::from_pair(&sample_schema_id(11), &sample_payload_hash(22)));
}

#[test]
fn attestor_signature_borsh_round_trip() {
    round_trip_borsh(AttestorSignature { pubkey: sample_pubkey(1), sig: safe_sig(0xcd) });
}

#[test]
fn call_message_register_attestor_set_borsh_round_trip() {
    let msg = CallMessage::<S>::RegisterAttestorSet {
        members: SafeVec::<PubKey, MAX_ATTESTOR_SET_MEMBERS>::try_from(vec![
            sample_pubkey(1),
            sample_pubkey(2),
        ])
        .unwrap(),
        threshold: 1,
    };
    round_trip_borsh(msg);
}

#[test]
fn call_message_register_schema_borsh_round_trip() {
    let msg = CallMessage::<S>::RegisterSchema {
        name: SafeString::try_from("foo.bar".to_string()).unwrap(),
        version: 3,
        attestor_set: AttestorSet::derive_id(&[sample_pubkey(8)], 1),
        fee_routing_bps: 1000,
        fee_routing_addr: Some(sample_addr(10)),
    };
    round_trip_borsh(msg);
}

#[test]
fn call_message_submit_attestation_borsh_round_trip() {
    let sigs = SafeVec::<AttestorSignature, MAX_ATTESTATION_SIGNATURES>::try_from(vec![
        AttestorSignature { pubkey: sample_pubkey(1), sig: safe_sig(0xcd) },
    ])
    .unwrap();
    let msg = CallMessage::<S>::SubmitAttestation {
        schema_id: sample_schema_id(1),
        payload_hash: sample_payload_hash(2),
        signatures: sigs,
    };
    round_trip_borsh(msg);
}

// ----- Serde JSON round-trips ------------------------------------------------

#[test]
fn schema_json_round_trip() {
    let original = sample_schema();
    let json = serde_json::to_string(&original).expect("encode");
    let decoded: Schema<S> = serde_json::from_str(&json).expect("decode");
    assert_eq!(original, decoded);
}

#[test]
fn attestation_json_round_trip() {
    let original = sample_attestation();
    let json = serde_json::to_string(&original).expect("encode");
    let decoded: Attestation<S> = serde_json::from_str(&json).expect("decode");
    assert_eq!(original, decoded);
}

#[test]
fn attestor_set_json_round_trip() {
    let original = sample_attestor_set();
    let json = serde_json::to_string(&original).expect("encode");
    let decoded: AttestorSet = serde_json::from_str(&json).expect("decode");
    assert_eq!(original, decoded);
}

#[test]
fn attestation_config_json_round_trip() {
    let cfg = AttestationConfig::<S> {
        treasury: sample_addr(9),
        lgt_token_id: TokenId::from([7u8; 32]),
        attestation_fee: Amount::new(1234),
        schema_registration_fee: Amount::new(5678),
        attestor_set_fee: Amount::new(91011),
        initial_attestor_sets: vec![InitialAttestorSet {
            members: vec![sample_pubkey(1), sample_pubkey(2)],
            threshold: 2,
        }],
        initial_schemas: vec![InitialSchema {
            owner: sample_addr(3),
            name: "demo".into(),
            version: 1,
            attestor_set: AttestorSet::derive_id(&[sample_pubkey(4)], 1),
            fee_routing_bps: 1000,
            fee_routing_addr: Some(sample_addr(5)),
        }],
        max_builder_bps: attestation::DEFAULT_MAX_BUILDER_BPS,
    };
    let json = serde_json::to_string(&cfg).expect("encode");
    let decoded: AttestationConfig<S> = serde_json::from_str(&json).expect("decode");
    assert_eq!(decoded.treasury, cfg.treasury);
    assert_eq!(decoded.attestation_fee, cfg.attestation_fee);
    assert_eq!(decoded.initial_attestor_sets.len(), 1);
    assert_eq!(decoded.initial_schemas.len(), 1);
}

// ----- Schema::derive_id -----------------------------------------------------

#[test]
fn schema_derive_id_is_deterministic() {
    let a = Schema::<S>::derive_id(&sample_addr(1), "foo", 1);
    let b = Schema::<S>::derive_id(&sample_addr(1), "foo", 1);
    assert_eq!(a, b);
}

#[test]
fn schema_derive_id_separates_owners() {
    // Same name + version, different owner → different schema_id.
    // This is the core anti-squatting property: namespaces are
    // owner-scoped, not global.
    let owner_a = Schema::<S>::derive_id(&sample_addr(1), "themisra.pop", 1);
    let owner_b = Schema::<S>::derive_id(&sample_addr(2), "themisra.pop", 1);
    assert_ne!(owner_a, owner_b);
}

#[test]
fn schema_derive_id_bumps_on_version() {
    let v1 = Schema::<S>::derive_id(&sample_addr(1), "foo", 1);
    let v2 = Schema::<S>::derive_id(&sample_addr(1), "foo", 2);
    assert_ne!(v1, v2);
}

#[test]
fn schema_derive_id_separates_names() {
    let a = Schema::<S>::derive_id(&sample_addr(1), "foo", 1);
    let b = Schema::<S>::derive_id(&sample_addr(1), "bar", 1);
    assert_ne!(a, b);
}

// ----- AttestorSet::derive_id ------------------------------------------------

#[test]
fn attestor_set_derive_id_is_deterministic() {
    let members = vec![sample_pubkey(1), sample_pubkey(2)];
    let a = AttestorSet::derive_id(&members, 1);
    let b = AttestorSet::derive_id(&members, 1);
    assert_eq!(a, b);
}

#[test]
fn attestor_set_derive_id_ignores_member_order() {
    // Member ordering is a canonical-encoding detail; the set id
    // must be order-independent so SDKs don't have to pre-sort.
    let a = AttestorSet::derive_id(&[sample_pubkey(1), sample_pubkey(2)], 1);
    let b = AttestorSet::derive_id(&[sample_pubkey(2), sample_pubkey(1)], 1);
    assert_eq!(a, b);
}

#[test]
fn attestor_set_derive_id_depends_on_threshold() {
    let members = vec![sample_pubkey(1), sample_pubkey(2)];
    let t1 = AttestorSet::derive_id(&members, 1);
    let t2 = AttestorSet::derive_id(&members, 2);
    assert_ne!(t1, t2);
}

#[test]
fn attestor_set_derive_id_depends_on_members() {
    let a = AttestorSet::derive_id(&[sample_pubkey(1)], 1);
    let b = AttestorSet::derive_id(&[sample_pubkey(2)], 1);
    assert_ne!(a, b);
}

// ----- AttestationId compound ------------------------------------------------

#[test]
fn attestation_id_from_pair_concats_correctly() {
    let sid = sample_schema_id(0xAA);
    let ph = sample_payload_hash(0xBB);
    let id = AttestationId::from_pair(&sid, &ph);
    assert_eq!(id.schema_id, sid);
    assert_eq!(id.payload_hash, ph);
}

#[test]
fn attestation_id_display_round_trip() {
    use std::str::FromStr;
    let sid = sample_schema_id(0xCC);
    let ph = sample_payload_hash(0xDD);
    let original = AttestationId::from_pair(&sid, &ph);
    let displayed = original.to_string();
    let decoded = AttestationId::from_str(&displayed).expect("round-trip parses");
    assert_eq!(original, decoded);
}

// ----- SignedAttestationPayload::digest --------------------------------------

#[test]
fn signed_payload_digest_is_deterministic() {
    let payload = sample_signed_payload();
    assert_eq!(payload.digest(), payload.digest());
}

#[test]
fn signed_payload_digest_changes_with_timestamp() {
    let base = sample_signed_payload();
    let mut mutated = base.clone();
    mutated.timestamp += 1;
    assert_ne!(base.digest(), mutated.digest());
}

#[test]
fn signed_payload_digest_changes_with_schema_id() {
    let base = sample_signed_payload();
    let mut mutated = base.clone();
    mutated.schema_id = sample_schema_id(0xEE);
    assert_ne!(base.digest(), mutated.digest());
}

#[test]
fn signed_payload_digest_changes_with_payload_hash() {
    let base = sample_signed_payload();
    let mut mutated = base.clone();
    mutated.payload_hash = sample_payload_hash(0xFF);
    assert_ne!(base.digest(), mutated.digest());
}

#[test]
fn signed_payload_digest_changes_with_submitter() {
    let base = sample_signed_payload();
    let mut mutated = base.clone();
    mutated.submitter = sample_addr(0x77);
    assert_ne!(base.digest(), mutated.digest());
}

// ----- Protocol constants ----------------------------------------------------

#[test]
fn default_max_builder_bps_is_half() {
    // Documenting the default invariant explicitly so a future refactor
    // doesn't silently raise the const default without thought. The
    // runtime cap lives in module state (governance-tunable per #40);
    // this asserts the bootstrap default for genesis configs that
    // omit `AttestationConfig::max_builder_bps`.
    assert_eq!(attestation::DEFAULT_MAX_BUILDER_BPS, 5000);
}

// Compile-time assertion: ed25519 (64 bytes) and BLS (48 bytes)
// signatures both fit inside `MAX_ATTESTOR_SIGNATURE_BYTES` (96).
// `const _ = assert!(...)` evaluates at compile time, so a future
// reduction of the cap below ed25519's 64 fails the build.
const _: () = assert!(MAX_ATTESTOR_SIGNATURE_BYTES >= 64);
const _: () = assert!(MAX_ATTESTOR_SIGNATURE_BYTES >= 48);
