//! Borsh wire-format snapshot tests for the public protocol types.
//!
//! Every type that crosses a chain boundary (call messages submitted in
//! transactions, state values stored on-chain, signed payloads consumed
//! by attestors) is hashed here against a checked-in expected
//! encoding. A PR that accidentally shifts a field, reorders a struct,
//! or changes a serialization derive flips the assertion red — and the
//! diff in this file makes the wire-format change explicit so reviewers
//! catch it deliberately.
//!
//! ## When this fires
//!
//! Common causes: adding a field to a struct, reordering fields,
//! changing a field's type, swapping a `Vec<T>` cap, adding a new
//! `CallMessage` variant in any position other than the last (Borsh
//! enums are tag-prefixed so appending a variant is safe; reordering
//! is a hard fork).
//!
//! ## When this fails
//!
//! Two paths:
//!
//! 1. **Unintended change.** Revert the diff. The assertion was doing
//!    its job.
//! 2. **Intended change.** Update the expected hex literal with the
//!    new value AND add a comment in the surrounding test explaining
//!    the wire-format impact. If the change breaks any existing
//!    on-chain state (state values, attestation signatures), add a
//!    migration plan to the PR description.
//!
//! ## Test data convention
//!
//! Each value uses distinctive byte patterns so the encoded bytes
//! tell a story when read by hand:
//!
//! - `0x11..` → SchemaId
//! - `0x22..` → AttestorSetId
//! - `0x33..` → PayloadHash
//! - `0xAA..`, `0xBB..`, `0xCC..` → PubKeys (three of them)
//! - `0xDD..`, `0xEE..` → AttestorSignature `sig` content
//! - `0x77..` → submitter address
//! - `0x88..` → fee_routing_addr
//!
//! Strings: `"ligate.test"` for schema name; short, ASCII, no
//! locale-sensitive surprises.

use attestation::{
    Attestation, AttestationId, AttestorSet, AttestorSetId, AttestorSignature, CallMessage,
    PayloadHash, PubKey, Schema, SchemaId, SignedAttestationPayload, MAX_ATTESTATION_SIGNATURES,
    MAX_ATTESTOR_SET_MEMBERS, MAX_ATTESTOR_SIGNATURE_BYTES,
};
use borsh::BorshSerialize;
use sov_modules_api::{Address, SafeString, SafeVec};

type S = sov_test_utils::TestSpec;

// --- helpers ---------------------------------------------------------------

/// Convert a byte slice to a lowercase hex string with no separators.
fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Borsh-serialize `value`, hex-encode, assert against `expected_hex`.
/// On mismatch, print a clear actionable error pointing the reader at
/// the file-level docs.
fn assert_snapshot<T: BorshSerialize>(name: &str, value: &T, expected_hex: &str) {
    let bytes = borsh::to_vec(value).expect("Borsh serialization of owned data is infallible");
    let actual_hex = to_hex(&bytes);
    assert_eq!(
        actual_hex, expected_hex,
        "\nBorsh snapshot for `{name}` changed.\n\
         \n\
         If this change is intentional (you bumped a protocol type, added a\n\
         field, reordered a variant), update the expected hex literal with\n\
         the new value AND document the wire-format impact in the PR. If a\n\
         migration is required for any existing on-chain state, capture\n\
         that in the PR description before merging.\n\
         \n\
         Otherwise, revert the change.\n\
         \n\
         expected: {expected_hex}\n\
         actual:   {actual_hex}\n",
    );
}

// --- 32-byte ID types ------------------------------------------------------
//
// All four are macro-generated newtypes around `[u8; 32]`. The Borsh
// encoding is just the 32 bytes verbatim. Snapshotting them anyway so
// the test fires if anyone changes the underlying primitive (e.g. moves
// to a length-prefixed encoding).

#[test]
fn snapshot_schema_id() {
    assert_snapshot(
        "SchemaId",
        &SchemaId::from([0x11u8; 32]),
        "1111111111111111111111111111111111111111111111111111111111111111",
    );
}

#[test]
fn snapshot_attestor_set_id() {
    assert_snapshot(
        "AttestorSetId",
        &AttestorSetId::from([0x22u8; 32]),
        "2222222222222222222222222222222222222222222222222222222222222222",
    );
}

#[test]
fn snapshot_payload_hash() {
    assert_snapshot(
        "PayloadHash",
        &PayloadHash::from([0x33u8; 32]),
        "3333333333333333333333333333333333333333333333333333333333333333",
    );
}

#[test]
fn snapshot_pubkey() {
    assert_snapshot(
        "PubKey",
        &PubKey::from([0xAAu8; 32]),
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    );
}

// --- AttestationId compound id ---------------------------------------------

#[test]
fn snapshot_attestation_id() {
    let id =
        AttestationId::from_pair(&SchemaId::from([0x11u8; 32]), &PayloadHash::from([0x33u8; 32]));
    assert_snapshot(
        "AttestationId",
        &id,
        "1111111111111111111111111111111111111111111111111111111111111111\
         3333333333333333333333333333333333333333333333333333333333333333",
    );
}

// --- AttestorSet ----------------------------------------------------------

#[test]
fn snapshot_attestor_set() {
    let set = AttestorSet {
        members: vec![
            PubKey::from([0xAAu8; 32]),
            PubKey::from([0xBBu8; 32]),
            PubKey::from([0xCCu8; 32]),
        ],
        threshold: 2,
    };
    assert_snapshot(
        "AttestorSet",
        &set,
        // Borsh: u32 len (3 = 0x03000000 LE) + 3 × 32-byte members + u8 threshold (0x02)
        "03000000\
         aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\
         bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\
         cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc\
         02",
    );
}

// --- AttestorSignature ----------------------------------------------------

#[test]
fn snapshot_attestor_signature() {
    let sig = AttestorSignature {
        pubkey: PubKey::from([0xAAu8; 32]),
        sig: SafeVec::<u8, MAX_ATTESTOR_SIGNATURE_BYTES>::try_from(vec![0xDDu8; 64])
            .expect("64 bytes fits MAX_ATTESTOR_SIGNATURE_BYTES (96)"),
    };
    assert_snapshot(
        "AttestorSignature",
        &sig,
        // Borsh: 32-byte pubkey + u32 sig len (64 = 0x40000000 LE) + 64 bytes of 0xDD
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\
         40000000\
         dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd\
         dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
    );
}

// --- Schema<S> ------------------------------------------------------------

#[test]
fn snapshot_schema() {
    let schema: Schema<S> = Schema {
        owner: Address::from([0x77u8; 28]),
        name: String::from("ligate.test"),
        version: 1,
        attestor_set: AttestorSetId::from([0x22u8; 32]),
        fee_routing_bps: 0,
        fee_routing_addr: None,
    };
    assert_snapshot(
        "Schema<TestSpec>",
        &schema,
        // owner (28 bytes) + name (u32 len = 11 + "ligate.test" UTF-8) +
        // version (u32 LE) + attestor_set (32 bytes) + fee_routing_bps (u16 LE) +
        // Option<Address>::None (1 byte tag)
        "77777777777777777777777777777777777777777777777777777777\
         0b0000006c69676174652e74657374\
         01000000\
         2222222222222222222222222222222222222222222222222222222222222222\
         0000\
         00",
    );
}

// --- Attestation<S> -------------------------------------------------------

#[test]
fn snapshot_attestation() {
    let att: Attestation<S> = Attestation {
        schema_id: SchemaId::from([0x11u8; 32]),
        payload_hash: PayloadHash::from([0x33u8; 32]),
        submitter: Address::from([0x77u8; 28]),
        timestamp: 0,
        signatures: vec![AttestorSignature {
            pubkey: PubKey::from([0xAAu8; 32]),
            sig: SafeVec::<u8, MAX_ATTESTOR_SIGNATURE_BYTES>::try_from(vec![0xDDu8; 64]).unwrap(),
        }],
    };
    assert_snapshot(
        "Attestation<TestSpec>",
        &att,
        // schema_id (32) + payload_hash (32) + submitter (28) + timestamp (u64 LE = 0) +
        // signatures (u32 len = 1) + 1 × AttestorSignature (32 + u32 len 64 + 64 bytes)
        "1111111111111111111111111111111111111111111111111111111111111111\
         3333333333333333333333333333333333333333333333333333333333333333\
         77777777777777777777777777777777777777777777777777777777\
         0000000000000000\
         01000000\
         aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\
         40000000\
         dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd\
         dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
    );
}

// --- SignedAttestationPayload<S> ------------------------------------------
//
// This is the payload attestors sign over. The encoding is load-bearing
// for cross-attestor signature compatibility — a change here invalidates
// every signature an off-chain attestor has ever produced.

#[test]
fn snapshot_signed_attestation_payload() {
    let payload: SignedAttestationPayload<S> = SignedAttestationPayload {
        schema_id: SchemaId::from([0x11u8; 32]),
        payload_hash: PayloadHash::from([0x33u8; 32]),
        submitter: Address::from([0x77u8; 28]),
        timestamp: 0,
    };
    assert_snapshot(
        "SignedAttestationPayload<TestSpec>",
        &payload,
        // schema_id (32) + payload_hash (32) + submitter (28) + timestamp (u64 LE = 0)
        "1111111111111111111111111111111111111111111111111111111111111111\
         3333333333333333333333333333333333333333333333333333333333333333\
         77777777777777777777777777777777777777777777777777777777\
         0000000000000000",
    );
}

// --- CallMessage<S> variants ---------------------------------------------
//
// The call enum is the chain's user-facing transaction surface. Variant
// **order** is part of the wire format (Borsh prefixes each variant
// with a u8 tag = its declaration index). Adding a variant in the
// middle is a hard fork; appending is safe.

#[test]
fn snapshot_call_message_register_attestor_set() {
    let msg: CallMessage<S> = CallMessage::RegisterAttestorSet {
        members: SafeVec::<PubKey, MAX_ATTESTOR_SET_MEMBERS>::try_from(vec![
            PubKey::from([0xAAu8; 32]),
            PubKey::from([0xBBu8; 32]),
            PubKey::from([0xCCu8; 32]),
        ])
        .unwrap(),
        threshold: 2,
    };
    assert_snapshot(
        "CallMessage::RegisterAttestorSet",
        &msg,
        // variant tag (0x00) + u32 len (3) + 3 × 32-byte pubkeys + u8 threshold (0x02)
        "00\
         03000000\
         aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\
         bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\
         cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc\
         02",
    );
}

#[test]
fn snapshot_call_message_register_schema() {
    let msg: CallMessage<S> = CallMessage::RegisterSchema {
        name: SafeString::try_from(String::from("ligate.test")).unwrap(),
        version: 1,
        attestor_set: AttestorSetId::from([0x22u8; 32]),
        fee_routing_bps: 5000,
        fee_routing_addr: Some(Address::from([0x88u8; 28])),
    };
    assert_snapshot(
        "CallMessage::RegisterSchema",
        &msg,
        // variant tag (0x01) + name (u32 len 11 + "ligate.test") +
        // version (u32 LE = 1) + attestor_set (32) +
        // fee_routing_bps (u16 LE = 5000 = 0x88 0x13) +
        // Option<Address>::Some(...) (tag 0x01 + 28 bytes)
        "01\
         0b0000006c69676174652e74657374\
         01000000\
         2222222222222222222222222222222222222222222222222222222222222222\
         8813\
         01\
         88888888888888888888888888888888888888888888888888888888",
    );
}

#[test]
fn snapshot_call_message_submit_attestation() {
    let msg: CallMessage<S> = CallMessage::SubmitAttestation {
        schema_id: SchemaId::from([0x11u8; 32]),
        payload_hash: PayloadHash::from([0x33u8; 32]),
        signatures: SafeVec::<AttestorSignature, MAX_ATTESTATION_SIGNATURES>::try_from(vec![
            AttestorSignature {
                pubkey: PubKey::from([0xAAu8; 32]),
                sig: SafeVec::<u8, MAX_ATTESTOR_SIGNATURE_BYTES>::try_from(vec![0xDDu8; 64])
                    .unwrap(),
            },
        ])
        .unwrap(),
    };
    assert_snapshot(
        "CallMessage::SubmitAttestation",
        &msg,
        // variant tag (0x02) + schema_id (32) + payload_hash (32) +
        // signatures (u32 len 1) + 1 × AttestorSignature (32 + u32 len 64 + 64 bytes)
        "02\
         1111111111111111111111111111111111111111111111111111111111111111\
         3333333333333333333333333333333333333333333333333333333333333333\
         01000000\
         aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\
         40000000\
         dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd\
         dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
    );
}
