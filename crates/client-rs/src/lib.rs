//! Rust client SDK for the Ligate attestation protocol.
//!
//! This crate provides typed builders for the three state-changing
//! [`CallMessage`] variants accepted by the on-chain attestation module, plus
//! helpers for computing the canonical attestation digest and producing
//! ed25519 signatures over it.
//!
//! Actual JSON-RPC transport (submitting signed transactions to a Ligate
//! node) lands in a separate issue (#21); the RPC endpoints do not exist yet.
//! Today this crate is purely about typed message construction and signing,
//! the pieces a test client, relayer, or attestor would need before any
//! network code.
//!
//! # Re-exports
//!
//! The protocol types from the `attestation` crate are re-exported so
//! downstream consumers of the SDK do not need a direct path dependency on
//! the module crate.
//!
//! # Example
//!
//! ```no_run
//! use ed25519_dalek::SigningKey;
//! use ligate_client::{
//!     build_signed_submit, register_attestor_set, register_schema, AttestorSet, Schema,
//! };
//!
//! // Pretend these exist somewhere in your wallet.
//! let signers: Vec<SigningKey> =
//!     (1u8..=3).map(|i| SigningKey::from_bytes(&[i; 32])).collect();
//! let members: Vec<[u8; 32]> =
//!     signers.iter().map(|sk| sk.verifying_key().to_bytes()).collect();
//!
//! // 1. Build a RegisterAttestorSet message.
//! let _msg = register_attestor_set(members.clone(), 2);
//!
//! // 2. Build a RegisterSchema message (fee routing omitted).
//! let attestor_set_id = AttestorSet::derive_id(&members, 2);
//! let _msg = register_schema("themisra.proof-of-prompt".into(), 1, attestor_set_id, 0, None);
//!
//! // 3. Build a SubmitAttestation message signed by a 2-of-3 quorum.
//! let submitter = [1u8; 32];
//! let schema_id = Schema::derive_id(&submitter, "themisra.proof-of-prompt", 1);
//! let payload_hash = [0x42u8; 32];
//! let keys: Vec<&SigningKey> = signers.iter().take(2).collect();
//! let _msg = build_signed_submit(&keys, schema_id, payload_hash, submitter, 0);
//! ```

#![deny(missing_docs)]

use borsh::BorshSerialize;
use ed25519_dalek::{Signer, SigningKey};
use thiserror::Error as ThisError;

pub use attestation::{
    Address, Attestation, AttestationKey, AttestorSet, AttestorSetId, AttestorSignature,
    CallMessage, Hash32, PubKey, Schema, SchemaId, SignedAttestationPayload,
};

// ----- Errors -----------------------------------------------------------------

/// Errors surfaced by the Ligate client SDK.
///
/// Reserved for future transport errors (JSON-RPC, signing adapters, etc).
/// Empty today so the crate surface already has a stable typed error that
/// downstream callers can depend on.
#[derive(Debug, ThisError)]
pub enum ClientError {}

// ----- Builders ---------------------------------------------------------------

/// Build a [`CallMessage::RegisterAttestorSet`].
///
/// The state transition re-derives [`AttestorSetId`] from `members` and
/// `threshold` on submission; callers do not need to precompute it. Members
/// are stored in sorted order on-chain regardless of submission order.
pub fn register_attestor_set(members: Vec<PubKey>, threshold: u8) -> CallMessage {
    CallMessage::RegisterAttestorSet { members, threshold }
}

/// Build a [`CallMessage::RegisterSchema`].
///
/// `fee_routing_addr` must be `Some` iff `fee_routing_bps > 0`, and
/// `fee_routing_bps` must be `<= attestation::MAX_BUILDER_BPS` (5000 in v0).
/// The state transition enforces these invariants; this builder is a thin
/// typed wrapper around the enum variant.
pub fn register_schema(
    name: String,
    version: u32,
    attestor_set: AttestorSetId,
    fee_routing_bps: u16,
    fee_routing_addr: Option<Address>,
) -> CallMessage {
    CallMessage::RegisterSchema { name, version, attestor_set, fee_routing_bps, fee_routing_addr }
}

/// Build a [`CallMessage::SubmitAttestation`].
///
/// Signatures must be over the canonical [`SignedAttestationPayload`] digest
/// for this `(schema_id, payload_hash, submitter, timestamp)` tuple. Use
/// [`attestation_digest`] to compute the exact digest and [`sign_attestation`]
/// to sign it, or [`build_signed_submit`] for the combined flow.
pub fn submit_attestation(
    schema_id: SchemaId,
    payload_hash: Hash32,
    signatures: Vec<AttestorSignature>,
) -> CallMessage {
    CallMessage::SubmitAttestation { schema_id, payload_hash, signatures }
}

// ----- Digest + signing -------------------------------------------------------

/// Compute the canonical attestation digest for the given tuple.
///
/// Constructs a [`SignedAttestationPayload`] with those fields and returns
/// its `.digest()`, which is SHA-256 over the canonical Borsh encoding. This
/// is the exact digest every attestor must sign over; the on-chain module
/// recomputes it at submit time from the schema id, payload hash, message
/// sender, and block timestamp, then verifies each signature against it.
pub fn attestation_digest(
    schema_id: SchemaId,
    payload_hash: Hash32,
    submitter: Address,
    timestamp: u64,
) -> Hash32 {
    SignedAttestationPayload { schema_id, payload_hash, submitter, timestamp }.digest()
}

/// Sign an attestation digest with an ed25519 key.
///
/// Returns an [`AttestorSignature`] whose `pubkey` is the signing key's
/// verifying key bytes and whose `sig` is the 64-byte ed25519 signature. The
/// produced signature verifies under `ed25519_dalek::Verifier::verify`
/// against `digest` and the same verifying key.
pub fn sign_attestation(signing_key: &SigningKey, digest: &Hash32) -> AttestorSignature {
    let signature = signing_key.sign(digest);
    AttestorSignature {
        pubkey: signing_key.verifying_key().to_bytes(),
        sig: signature.to_bytes().to_vec(),
    }
}

/// Convenience: compute the digest, sign with every key, and return a
/// [`CallMessage::SubmitAttestation`] ready to submit.
///
/// This is the common relayer or test-client flow: take a quorum of signing
/// keys, the submission tuple, and produce the final call message in one
/// step. Each key must correspond to a member of the schema's attestor set,
/// and the total number of signatures must meet the set's threshold; both
/// are enforced server-side by the state transition.
pub fn build_signed_submit(
    signing_keys: &[&SigningKey],
    schema_id: SchemaId,
    payload_hash: Hash32,
    submitter: Address,
    timestamp: u64,
) -> CallMessage {
    let digest = attestation_digest(schema_id, payload_hash, submitter, timestamp);
    let signatures =
        signing_keys.iter().map(|sk| sign_attestation(sk, &digest)).collect::<Vec<_>>();
    submit_attestation(schema_id, payload_hash, signatures)
}

// Suppress "unused" lint on BorshSerialize; it's a transitive requirement
// of the re-exported types and pulled in so doctests compile cleanly if a
// downstream user encodes a CallMessage directly.
#[allow(dead_code)]
fn _assert_borsh_available<T: BorshSerialize>() {}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use ed25519_dalek::{Verifier, VerifyingKey};

    use super::*;

    fn sample_signer(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn sample_address(n: u8) -> Address {
        [n; 32]
    }

    fn sample_pubkey(n: u8) -> PubKey {
        [n; 32]
    }

    // ------ Builders ------------------------------------------------------

    #[test]
    fn register_attestor_set_builds_correct_variant() {
        let members = vec![sample_pubkey(1), sample_pubkey(2), sample_pubkey(3)];
        let msg = register_attestor_set(members.clone(), 2);
        match msg {
            CallMessage::RegisterAttestorSet { members: got_members, threshold } => {
                assert_eq!(got_members, members);
                assert_eq!(threshold, 2);
            }
            other => panic!("expected RegisterAttestorSet, got {other:?}"),
        }
    }

    #[test]
    fn register_schema_builds_correct_variant() {
        let attestor_set_id = [7u8; 32];
        let fee_addr = sample_address(9);
        let msg = register_schema(
            "themisra.proof-of-prompt".to_string(),
            3,
            attestor_set_id,
            2500,
            Some(fee_addr),
        );
        match msg {
            CallMessage::RegisterSchema {
                name,
                version,
                attestor_set,
                fee_routing_bps,
                fee_routing_addr,
            } => {
                assert_eq!(name, "themisra.proof-of-prompt");
                assert_eq!(version, 3);
                assert_eq!(attestor_set, attestor_set_id);
                assert_eq!(fee_routing_bps, 2500);
                assert_eq!(fee_routing_addr, Some(fee_addr));
            }
            other => panic!("expected RegisterSchema, got {other:?}"),
        }
    }

    #[test]
    fn submit_attestation_builds_correct_variant() {
        let schema_id = [1u8; 32];
        let payload_hash = [2u8; 32];
        let signatures = vec![AttestorSignature { pubkey: sample_pubkey(1), sig: vec![0xab; 64] }];
        let msg = submit_attestation(schema_id, payload_hash, signatures.clone());
        match msg {
            CallMessage::SubmitAttestation {
                schema_id: got_schema_id,
                payload_hash: got_payload_hash,
                signatures: got_signatures,
            } => {
                assert_eq!(got_schema_id, schema_id);
                assert_eq!(got_payload_hash, payload_hash);
                assert_eq!(got_signatures, signatures);
            }
            other => panic!("expected SubmitAttestation, got {other:?}"),
        }
    }

    // ------ Digest helper -------------------------------------------------

    #[test]
    fn attestation_digest_matches_struct_digest() {
        let schema_id = [1u8; 32];
        let payload_hash = [2u8; 32];
        let submitter = sample_address(3);
        let timestamp = 1_700_000_000u64;

        let via_helper = attestation_digest(schema_id, payload_hash, submitter, timestamp);
        let via_struct =
            SignedAttestationPayload { schema_id, payload_hash, submitter, timestamp }.digest();

        assert_eq!(via_helper, via_struct);
    }

    #[test]
    fn attestation_digest_is_field_sensitive() {
        let base = attestation_digest([1u8; 32], [2u8; 32], sample_address(3), 1_000);
        // Any single-field change must yield a different digest.
        assert_ne!(base, attestation_digest([9u8; 32], [2u8; 32], sample_address(3), 1_000));
        assert_ne!(base, attestation_digest([1u8; 32], [9u8; 32], sample_address(3), 1_000));
        assert_ne!(base, attestation_digest([1u8; 32], [2u8; 32], sample_address(9), 1_000));
        assert_ne!(base, attestation_digest([1u8; 32], [2u8; 32], sample_address(3), 1_001));
    }

    // ------ sign_attestation ----------------------------------------------

    #[test]
    fn sign_attestation_produces_verifiable_signature() {
        let signer = sample_signer(7);
        let digest = attestation_digest([1u8; 32], [2u8; 32], sample_address(3), 42);

        let sig = sign_attestation(&signer, &digest);

        // pubkey matches the key's verifying key bytes
        assert_eq!(sig.pubkey, signer.verifying_key().to_bytes());
        // signature is the ed25519 expected length (64 bytes)
        assert_eq!(sig.sig.len(), 64);

        // And it verifies.
        let vk = VerifyingKey::from_bytes(&sig.pubkey).expect("pubkey should be valid ed25519");
        let sig_bytes: [u8; 64] =
            sig.sig.as_slice().try_into().expect("signature should be 64 bytes");
        let signature = ed25519_dalek::Signature::from_bytes(&sig_bytes);
        vk.verify(&digest, &signature).expect("signature should verify");
    }

    #[test]
    fn sign_attestation_rejects_wrong_digest_on_verify() {
        // Sanity: the signature binds to the digest, flipping a byte in the
        // digest after signing breaks verification.
        let signer = sample_signer(7);
        let digest = attestation_digest([1u8; 32], [2u8; 32], sample_address(3), 42);
        let sig = sign_attestation(&signer, &digest);

        let mut tampered = digest;
        tampered[0] ^= 0x01;

        let vk = VerifyingKey::from_bytes(&sig.pubkey).unwrap();
        let sig_bytes: [u8; 64] = sig.sig.as_slice().try_into().unwrap();
        let signature = ed25519_dalek::Signature::from_bytes(&sig_bytes);
        assert!(vk.verify(&tampered, &signature).is_err());
    }

    // ------ build_signed_submit -------------------------------------------

    #[test]
    fn build_signed_submit_produces_valid_two_of_n_submission() {
        let signer_a = sample_signer(1);
        let signer_b = sample_signer(2);
        let keys: Vec<&SigningKey> = vec![&signer_a, &signer_b];

        let schema_id = [0xABu8; 32];
        let payload_hash = [0xCDu8; 32];
        let submitter = sample_address(5);
        let timestamp = 1_234_567_890u64;

        let msg = build_signed_submit(&keys, schema_id, payload_hash, submitter, timestamp);

        let (got_schema_id, got_payload_hash, signatures) = match msg {
            CallMessage::SubmitAttestation { schema_id, payload_hash, signatures } => {
                (schema_id, payload_hash, signatures)
            }
            other => panic!("expected SubmitAttestation, got {other:?}"),
        };
        assert_eq!(got_schema_id, schema_id);
        assert_eq!(got_payload_hash, payload_hash);
        assert_eq!(signatures.len(), 2);

        // Every signature must verify against the canonical digest.
        let digest = attestation_digest(schema_id, payload_hash, submitter, timestamp);
        for sig in &signatures {
            let vk = VerifyingKey::from_bytes(&sig.pubkey).expect("valid pubkey");
            let sig_bytes: [u8; 64] = sig.sig.as_slice().try_into().expect("64-byte signature");
            let signature = ed25519_dalek::Signature::from_bytes(&sig_bytes);
            vk.verify(&digest, &signature).expect("signature must verify");
        }

        // And the two signatures come from the two distinct signing keys.
        assert_eq!(signatures[0].pubkey, signer_a.verifying_key().to_bytes());
        assert_eq!(signatures[1].pubkey, signer_b.verifying_key().to_bytes());
    }

    #[test]
    fn build_signed_submit_with_zero_keys_is_empty_submission() {
        // Edge case, the builder itself does not enforce quorum. Zero
        // keys produces an empty signatures vector; the on-chain state
        // transition will reject below-threshold submissions.
        let keys: Vec<&SigningKey> = Vec::new();
        let msg = build_signed_submit(&keys, [1u8; 32], [2u8; 32], sample_address(3), 0);
        match msg {
            CallMessage::SubmitAttestation { signatures, .. } => assert!(signatures.is_empty()),
            other => panic!("expected SubmitAttestation, got {other:?}"),
        }
    }

    // ------ Re-exports smoke test -----------------------------------------

    #[test]
    fn re_exported_schema_derive_id_matches_upstream() {
        // The re-exports are the same types as in the `attestation` crate.
        let owner = sample_address(1);
        let id = Schema::derive_id(&owner, "foo", 1);
        let upstream = attestation::Schema::derive_id(&owner, "foo", 1);
        assert_eq!(id, upstream);
    }

    #[test]
    fn re_exported_attestor_set_derive_id_matches_upstream() {
        let members = vec![sample_pubkey(1), sample_pubkey(2)];
        let id = AttestorSet::derive_id(&members, 1);
        let upstream = attestation::AttestorSet::derive_id(&members, 1);
        assert_eq!(id, upstream);
    }
}
