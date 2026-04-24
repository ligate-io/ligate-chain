//! Themisra Proof-of-Prompt schema, v1.
//!
//! This crate pins Themisra's on-chain payload shape, the canonical
//! Borsh + SHA-256 hashing rule that produces an attestation's
//! `payload_hash`, the derived schema id, and thin helpers for
//! building submit-attestation messages + client-side verification.
//!
//! # How it relates to the protocol
//!
//! At the chain level, an `attestation::Attestation` stores only a
//! `payload_hash` (32 bytes). Themisra's payload (model id, prompt
//! hash, output hash, timestamp, client address) lives off-chain;
//! clients reconstruct it and re-hash it to check against the stored
//! attestation. The attestor quorum has co-signed the [`Attestation`]
//! at submit time, so the client's verification is a hash check, not
//! a trust decision.
//!
//! # How it relates to Themisra the product
//!
//! The Themisra backend (attestor quorum HTTP API, TS SDK, web app)
//! depends on this crate's types. So does any third-party app that
//! wants to accept or verify Themisra Proof-of-Prompt receipts.

#![deny(missing_docs)]

use attestation::{Address, Attestation, AttestorSignature, CallMessage, Hash32, Schema, SchemaId};
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error as ThisError;

/// Canonical schema name. Combined with the owner address and the
/// version below, produces the schema id Themisra registers on-chain.
pub const THEMISRA_POP_NAME: &str = "themisra.proof-of-prompt";

/// Current Themisra PoP schema version. Any breaking change to
/// [`ThemisraPopPayload`] must bump this, not mutate the existing
/// version in place.
pub const THEMISRA_POP_VERSION: u32 = 1;

/// Themisra's off-chain payload. Hashed to produce an attestation's
/// `payload_hash`.
///
/// The chain never sees the plaintext of this struct. Clients carry it
/// alongside the on-chain attestation id and use [`verify`] to confirm
/// the chain record anchors this exact payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct ThemisraPopPayload {
    /// Model identifier the attestation is bound to, e.g. `"claude-opus-4-7"`.
    pub model: String,
    /// SHA-256 of the raw prompt the client submitted off-chain.
    pub prompt_hash: Hash32,
    /// SHA-256 of the raw model output the client is attesting to.
    pub output_hash: Hash32,
    /// Unix seconds at which the client requested attestation.
    pub timestamp: u64,
    /// Address of the client the receipt belongs to.
    ///
    /// May differ from the transaction submitter. Example: a Themisra
    /// relayer submits an attestation on behalf of a user; submitter is
    /// the relayer, `client` is the user.
    pub client: Address,
}

/// Canonical hash of a Themisra PoP payload: `SHA-256(borsh(payload))`.
///
/// This is what goes into `Attestation.payload_hash` when Themisra
/// submits to the chain. Must be reproduced exactly on the verify side.
pub fn payload_hash(payload: &ThemisraPopPayload) -> Hash32 {
    let bytes = payload.try_to_vec().expect("Borsh serialization of owned data is infallible");
    Sha256::digest(&bytes).into()
}

/// Derive the Themisra PoP schema id given the schema owner's address.
///
/// Thin wrapper around [`Schema::derive_id`] that pins
/// [`THEMISRA_POP_NAME`] and [`THEMISRA_POP_VERSION`] so callers can't
/// typo them.
pub fn themisra_pop_schema_id(owner: &Address) -> SchemaId {
    Schema::derive_id(owner, THEMISRA_POP_NAME, THEMISRA_POP_VERSION)
}

/// Build a [`CallMessage::SubmitAttestation`] for a Themisra PoP
/// attestation.
///
/// Signatures must be collected off-chain from the Themisra attestor
/// quorum before calling this. This helper only formats the on-chain
/// message; it does not sign, fetch signatures, or submit.
pub fn build_submit_attestation(
    schema_id: SchemaId,
    payload: &ThemisraPopPayload,
    signatures: Vec<AttestorSignature>,
) -> CallMessage {
    CallMessage::SubmitAttestation { schema_id, payload_hash: payload_hash(payload), signatures }
}

/// Client-side verification: re-hash the caller's `expected_payload`
/// and confirm it matches the stored [`Attestation::payload_hash`].
///
/// This does not verify attestor signatures (the chain already did
/// that at submit time; a stored attestation is already signature
/// valid). It only confirms the on-chain record anchors the specific
/// payload the caller expects, i.e. that no one has swapped an
/// attestation for a different prompt or output under your nose.
pub fn verify(
    attestation: &Attestation,
    expected_payload: &ThemisraPopPayload,
) -> Result<(), VerifyError> {
    let expected_hash = payload_hash(expected_payload);
    if attestation.payload_hash != expected_hash {
        return Err(VerifyError::PayloadHashMismatch {
            expected: expected_hash,
            actual: attestation.payload_hash,
        });
    }
    Ok(())
}

/// Errors returned by [`verify`].
#[derive(Debug, ThisError)]
pub enum VerifyError {
    /// `expected_payload` hashed to a different digest than what the
    /// on-chain [`Attestation`] stores.
    #[error("payload hash mismatch: expected {expected:?}, got {actual:?}")]
    PayloadHashMismatch {
        /// Hash computed from the caller's expected payload.
        expected: Hash32,
        /// Hash stored on-chain.
        actual: Hash32,
    },
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_payload() -> ThemisraPopPayload {
        ThemisraPopPayload {
            model: "claude-opus-4-7".to_string(),
            prompt_hash: [1u8; 32],
            output_hash: [2u8; 32],
            timestamp: 1_700_000_000,
            client: [3u8; 32],
        }
    }

    fn sample_attestation(payload_hash: Hash32) -> Attestation {
        Attestation {
            schema_id: [9u8; 32],
            payload_hash,
            submitter: [3u8; 32],
            timestamp: 1_700_000_000,
            signatures: vec![],
        }
    }

    // ---- Payload round-trips ------------------------------------------------

    #[test]
    fn payload_borsh_round_trip() {
        let p = sample_payload();
        let bytes = p.try_to_vec().expect("encode");
        let decoded = ThemisraPopPayload::try_from_slice(&bytes).expect("decode");
        assert_eq!(p, decoded);
    }

    #[test]
    fn payload_json_round_trip() {
        let p = sample_payload();
        let json = serde_json::to_string(&p).expect("encode");
        let decoded: ThemisraPopPayload = serde_json::from_str(&json).expect("decode");
        assert_eq!(p, decoded);
    }

    // ---- payload_hash -------------------------------------------------------

    #[test]
    fn payload_hash_is_deterministic() {
        let p = sample_payload();
        assert_eq!(payload_hash(&p), payload_hash(&p));
    }

    #[test]
    fn payload_hash_changes_on_model() {
        let a = sample_payload();
        let mut b = a.clone();
        b.model.push('x');
        assert_ne!(payload_hash(&a), payload_hash(&b));
    }

    #[test]
    fn payload_hash_changes_on_prompt_hash() {
        let a = sample_payload();
        let mut b = a.clone();
        b.prompt_hash[0] ^= 0x01;
        assert_ne!(payload_hash(&a), payload_hash(&b));
    }

    #[test]
    fn payload_hash_changes_on_output_hash() {
        let a = sample_payload();
        let mut b = a.clone();
        b.output_hash[0] ^= 0x01;
        assert_ne!(payload_hash(&a), payload_hash(&b));
    }

    #[test]
    fn payload_hash_changes_on_timestamp() {
        let a = sample_payload();
        let mut b = a.clone();
        b.timestamp += 1;
        assert_ne!(payload_hash(&a), payload_hash(&b));
    }

    #[test]
    fn payload_hash_changes_on_client() {
        let a = sample_payload();
        let mut b = a.clone();
        b.client[0] ^= 0x01;
        assert_ne!(payload_hash(&a), payload_hash(&b));
    }

    // ---- schema id ----------------------------------------------------------

    #[test]
    fn schema_id_is_deterministic() {
        let owner = [42u8; 32];
        assert_eq!(themisra_pop_schema_id(&owner), themisra_pop_schema_id(&owner));
    }

    #[test]
    fn schema_id_depends_on_owner() {
        assert_ne!(themisra_pop_schema_id(&[1u8; 32]), themisra_pop_schema_id(&[2u8; 32]));
    }

    #[test]
    fn schema_id_matches_protocol_derivation() {
        // If the protocol's Schema::derive_id formula ever drifts from
        // what this crate embeds, THEMISRA_POP_NAME + VERSION, the test
        // fails and tells us to investigate.
        let owner = [7u8; 32];
        let expected = Schema::derive_id(&owner, THEMISRA_POP_NAME, THEMISRA_POP_VERSION);
        assert_eq!(themisra_pop_schema_id(&owner), expected);
    }

    // ---- build_submit_attestation -------------------------------------------

    #[test]
    fn build_submit_attestation_embeds_hash() {
        let payload = sample_payload();
        let expected_hash = payload_hash(&payload);
        let schema_id = [5u8; 32];
        let msg = build_submit_attestation(schema_id, &payload, vec![]);
        match msg {
            CallMessage::SubmitAttestation { schema_id: sid, payload_hash: ph, signatures } => {
                assert_eq!(sid, schema_id);
                assert_eq!(ph, expected_hash);
                assert!(signatures.is_empty());
            }
            _ => panic!("expected SubmitAttestation variant"),
        }
    }

    // ---- verify -------------------------------------------------------------

    #[test]
    fn verify_happy_path() {
        let payload = sample_payload();
        let att = sample_attestation(payload_hash(&payload));
        verify(&att, &payload).expect("matching payload should verify");
    }

    #[test]
    fn verify_rejects_hash_mismatch() {
        let payload = sample_payload();
        let mut different = payload.clone();
        different.model = "gpt-4o".to_string();
        let att = sample_attestation(payload_hash(&different));

        let err = verify(&att, &payload).unwrap_err();
        match err {
            VerifyError::PayloadHashMismatch { expected, actual } => {
                assert_eq!(expected, payload_hash(&payload));
                assert_eq!(actual, payload_hash(&different));
            }
        }
    }
}
