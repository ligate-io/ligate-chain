//! Rust client SDK for the Ligate attestation protocol.
//!
//! This crate provides typed builders for the three state-changing
//! [`CallMessage`] variants accepted by the on-chain attestation
//! module, plus helpers for computing the canonical attestation
//! digest and producing ed25519 signatures over it.
//!
//! Actual JSON-RPC transport (submitting signed transactions to a
//! Ligate node) lands in a separate issue (#21); the RPC endpoints
//! are still being wired up. Today this crate is purely about typed
//! message construction and signing: the pieces a test client,
//! relayer, or attestor would need before any network code.
//!
//! # Generic over `Spec`
//!
//! The Sovereign SDK parameterises chain-level types (addresses,
//! storage backends, signature schemes) over a `Spec` trait. Every
//! builder here is generic over `<S: Spec>` and produces a typed
//! `CallMessage<S>` ready to embed in a transaction. Concrete
//! callers (test harness, relayer, devnet CLI) pin `S` to a specific
//! `Spec` implementation, typically `DefaultSpec` for native chain
//! interaction or `sov_test_utils::TestSpec` for tests.
//!
//! # Bounded payload types
//!
//! `CallMessage` payloads use `SafeString` and `SafeVec<T, MAX>`
//! rather than unbounded `String` / `Vec<T>` because the SDK's
//! `UniversalWallet` derive requires fixed-size encodings on the
//! wire. The builders here take ergonomic `Vec` / `String` inputs
//! and return a [`ClientError`] if the input exceeds the wire-format
//! cap. Caps are constants on the `attestation` crate; bumping them
//! is a hard fork.
//!
//! # Re-exports
//!
//! Protocol types from the `attestation` crate are re-exported so
//! downstream consumers don't need a direct path dependency on the
//! module crate.
//!
//! # Example
//!
//! ```no_run
//! use ed25519_dalek::SigningKey;
//! use ligate_client::{
//!     build_signed_submit, register_attestor_set, register_schema, AttestorSet, PayloadHash,
//!     PubKey, Schema,
//! };
//! use sov_modules_api::{Address, Spec};
//!
//! type S = sov_test_utils::TestSpec;
//!
//! // Pretend these exist somewhere in your wallet.
//! let signers: Vec<SigningKey> = (1u8..=3).map(|i| SigningKey::from_bytes(&[i; 32])).collect();
//! let members: Vec<PubKey> =
//!     signers.iter().map(|sk| PubKey::from(sk.verifying_key().to_bytes())).collect();
//!
//! // 1. Build a RegisterAttestorSet message.
//! let _msg = register_attestor_set::<S>(members.clone(), 2).expect("members under cap");
//!
//! // 2. Build a RegisterSchema message (fee routing omitted).
//! let attestor_set_id = AttestorSet::derive_id(&members, 2);
//! let _msg = register_schema::<S>(
//!     "themisra.proof-of-prompt".into(),
//!     1,
//!     attestor_set_id,
//!     0,
//!     None,
//! )
//! .expect("name under cap");
//!
//! // 3. Build a SubmitAttestation message signed by a 2-of-3 quorum.
//! let submitter: <S as Spec>::Address = Address::from([1u8; 28]);
//! let schema_id = Schema::<S>::derive_id(&submitter, "themisra.proof-of-prompt", 1);
//! let payload_hash = PayloadHash::from([0x42u8; 32]);
//! let keys: Vec<&SigningKey> = signers.iter().take(2).collect();
//! let _msg = build_signed_submit::<S>(&keys, schema_id, payload_hash, submitter, 0)
//!     .expect("signatures under cap");
//! ```

#![deny(missing_docs)]

use ed25519_dalek::{Signer, SigningKey};
use sov_modules_api::{SafeString, SafeVec, Spec};
use thiserror::Error as ThisError;

pub use attestation::{
    Attestation, AttestationConfig, AttestationId, AttestorSet, AttestorSetId, AttestorSignature,
    CallMessage, Hash32, PayloadHash, PubKey, Schema, SchemaId, SignedAttestationPayload,
    DEFAULT_MAX_BUILDER_BPS, MAX_ATTESTATION_SIGNATURES, MAX_ATTESTOR_SET_MEMBERS,
    MAX_ATTESTOR_SIGNATURE_BYTES,
};

// ----- Errors -----------------------------------------------------------------

/// Errors surfaced by the Ligate client SDK.
#[derive(Debug, ThisError)]
#[non_exhaustive]
pub enum ClientError {
    /// Caller supplied a `Vec<PubKey>` longer than
    /// [`MAX_ATTESTOR_SET_MEMBERS`].
    #[error("attestor set has {actual} members, max {max}")]
    AttestorSetMembersOverCap {
        /// Length supplied.
        actual: usize,
        /// Hard cap.
        max: usize,
    },

    /// Caller supplied a `Vec<AttestorSignature>` longer than
    /// [`MAX_ATTESTATION_SIGNATURES`].
    #[error("attestation has {actual} signatures, max {max}")]
    AttestationSignaturesOverCap {
        /// Length supplied.
        actual: usize,
        /// Hard cap.
        max: usize,
    },

    /// Caller supplied a signature byte string longer than
    /// [`MAX_ATTESTOR_SIGNATURE_BYTES`].
    #[error("signature has {actual} bytes, max {max}")]
    SignatureBytesOverCap {
        /// Length supplied.
        actual: usize,
        /// Hard cap.
        max: usize,
    },

    /// Caller supplied a schema name longer than the SDK's
    /// `SafeString` default cap.
    #[error("schema name exceeds the SafeString default cap: {0}")]
    SchemaNameOverCap(String),
}

// ----- Builders ---------------------------------------------------------------

/// Build a [`CallMessage::RegisterAttestorSet`].
///
/// The state transition re-derives [`AttestorSetId`] from `members`
/// and `threshold` on submission; callers don't need to precompute
/// it. Members are stored in sorted order on-chain regardless of
/// submission order.
///
/// Returns [`ClientError::AttestorSetMembersOverCap`] if `members`
/// exceeds [`MAX_ATTESTOR_SET_MEMBERS`].
pub fn register_attestor_set<S: Spec>(
    members: Vec<PubKey>,
    threshold: u8,
) -> Result<CallMessage<S>, ClientError> {
    let actual = members.len();
    let members = SafeVec::<PubKey, MAX_ATTESTOR_SET_MEMBERS>::try_from(members).map_err(|_| {
        ClientError::AttestorSetMembersOverCap { actual, max: MAX_ATTESTOR_SET_MEMBERS }
    })?;
    Ok(CallMessage::RegisterAttestorSet { members, threshold })
}

/// Build a [`CallMessage::RegisterSchema`].
///
/// `fee_routing_addr` must be `Some` iff `fee_routing_bps > 0`, and
/// `fee_routing_bps` must be `<= attestation::DEFAULT_MAX_BUILDER_BPS`
/// (5000 in v0; the runtime cap lives in module state and is
/// governance-tunable). The state transition enforces these invariants; this
/// builder is a thin typed wrapper around the enum variant. Returns
/// [`ClientError::SchemaNameOverCap`] if `name` exceeds the
/// `SafeString` default cap.
pub fn register_schema<S: Spec>(
    name: String,
    version: u32,
    attestor_set: AttestorSetId,
    fee_routing_bps: u16,
    fee_routing_addr: Option<S::Address>,
) -> Result<CallMessage<S>, ClientError> {
    let safe_name =
        SafeString::try_from(name.clone()).map_err(|_| ClientError::SchemaNameOverCap(name))?;
    Ok(CallMessage::RegisterSchema {
        name: safe_name,
        version,
        attestor_set,
        fee_routing_bps,
        fee_routing_addr,
    })
}

/// Build a [`CallMessage::SubmitAttestation`].
///
/// Signatures must be over the canonical [`SignedAttestationPayload`]
/// digest for this `(schema_id, payload_hash, submitter, timestamp)`
/// tuple. Use [`attestation_digest`] to compute the exact digest and
/// [`sign_attestation`] to sign it, or [`build_signed_submit`] for
/// the combined flow. Returns
/// [`ClientError::AttestationSignaturesOverCap`] if `signatures`
/// exceeds [`MAX_ATTESTATION_SIGNATURES`].
pub fn submit_attestation<S: Spec>(
    schema_id: SchemaId,
    payload_hash: PayloadHash,
    signatures: Vec<AttestorSignature>,
) -> Result<CallMessage<S>, ClientError> {
    let actual = signatures.len();
    let signatures = SafeVec::<AttestorSignature, MAX_ATTESTATION_SIGNATURES>::try_from(signatures)
        .map_err(|_| ClientError::AttestationSignaturesOverCap {
            actual,
            max: MAX_ATTESTATION_SIGNATURES,
        })?;
    Ok(CallMessage::SubmitAttestation { schema_id, payload_hash, signatures })
}

// ----- Digest + signing -------------------------------------------------------

/// Compute the canonical attestation digest for the given tuple.
///
/// Constructs a [`SignedAttestationPayload`] with those fields and
/// returns its `.digest()`, which is SHA-256 over the canonical
/// Borsh encoding. This is the exact digest every attestor must sign
/// over; the on-chain module recomputes it at submit time from the
/// schema id, payload hash, message sender, and block timestamp,
/// then verifies each signature against it.
pub fn attestation_digest<S: Spec>(
    schema_id: SchemaId,
    payload_hash: PayloadHash,
    submitter: S::Address,
    timestamp: u64,
) -> Hash32 {
    SignedAttestationPayload::<S> { schema_id, payload_hash, submitter, timestamp }.digest()
}

/// Sign an attestation digest with an ed25519 key.
///
/// Returns an [`AttestorSignature`] whose `pubkey` is the signing
/// key's verifying-key bytes wrapped as a `PubKey` and whose `sig`
/// is the 64-byte ed25519 signature wrapped in a
/// `SafeVec<u8, MAX_ATTESTOR_SIGNATURE_BYTES>`. Infallible: ed25519
/// signatures are 64 bytes, which always fits in 96.
pub fn sign_attestation(signing_key: &SigningKey, digest: &Hash32) -> AttestorSignature {
    let signature = signing_key.sign(digest);
    let sig_bytes: Vec<u8> = signature.to_bytes().to_vec();
    AttestorSignature {
        pubkey: PubKey::from(signing_key.verifying_key().to_bytes()),
        // 64 bytes always fits in 96; unwrap is safe.
        sig: SafeVec::<u8, MAX_ATTESTOR_SIGNATURE_BYTES>::try_from(sig_bytes)
            .expect("ed25519 signature is 64 bytes; fits in 96"),
    }
}

/// Convenience: compute the digest, sign with every key, and return
/// a [`CallMessage::SubmitAttestation`] ready to submit.
///
/// This is the common relayer or test-client flow: take a quorum of
/// signing keys, the submission tuple, and produce the final call
/// message in one step. Each key must correspond to a member of the
/// schema's attestor set, and the total number of signatures must
/// meet the set's threshold; both are enforced server-side by the
/// state transition. Returns
/// [`ClientError::AttestationSignaturesOverCap`] if more than
/// [`MAX_ATTESTATION_SIGNATURES`] keys are supplied.
pub fn build_signed_submit<S: Spec>(
    signing_keys: &[&SigningKey],
    schema_id: SchemaId,
    payload_hash: PayloadHash,
    submitter: S::Address,
    timestamp: u64,
) -> Result<CallMessage<S>, ClientError> {
    let digest = attestation_digest::<S>(schema_id, payload_hash, submitter, timestamp);
    let signatures =
        signing_keys.iter().map(|sk| sign_attestation(sk, &digest)).collect::<Vec<_>>();
    submit_attestation::<S>(schema_id, payload_hash, signatures)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use ed25519_dalek::{Verifier, VerifyingKey};
    use sov_modules_api::Address;

    use super::*;

    type S = sov_test_utils::TestSpec;

    fn sample_signer(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn sample_address(n: u8) -> <S as Spec>::Address {
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

    // ------ Builders ------------------------------------------------------

    #[test]
    fn register_attestor_set_builds_correct_variant() {
        let members = vec![sample_pubkey(1), sample_pubkey(2), sample_pubkey(3)];
        let msg: CallMessage<S> =
            register_attestor_set::<S>(members.clone(), 2).expect("members under cap");
        match msg {
            CallMessage::RegisterAttestorSet { members: got_members, threshold } => {
                let got: Vec<PubKey> = got_members.into_iter().collect();
                assert_eq!(got, members);
                assert_eq!(threshold, 2);
            }
            other => panic!("expected RegisterAttestorSet, got {other:?}"),
        }
    }

    #[test]
    fn register_attestor_set_rejects_over_cap() {
        // MAX_ATTESTOR_SET_MEMBERS + 1 members.
        let too_many: Vec<PubKey> =
            (0..=MAX_ATTESTOR_SET_MEMBERS as u8).map(sample_pubkey).collect();
        let result = register_attestor_set::<S>(too_many, 2);
        match result {
            Err(ClientError::AttestorSetMembersOverCap { actual, max }) => {
                assert_eq!(actual, MAX_ATTESTOR_SET_MEMBERS + 1);
                assert_eq!(max, MAX_ATTESTOR_SET_MEMBERS);
            }
            other => panic!("expected AttestorSetMembersOverCap, got {other:?}"),
        }
    }

    #[test]
    fn submit_attestation_rejects_over_cap_signatures() {
        // MAX_ATTESTATION_SIGNATURES + 1 signatures. Pin the
        // `AttestationSignaturesOverCap` variant explicitly so a
        // future refactor that replaces the cap check with a
        // different error type fails this test loudly.
        let schema_id = sample_schema_id(1);
        let payload_hash = sample_payload_hash(2);
        let too_many: Vec<AttestorSignature> = (0..=MAX_ATTESTATION_SIGNATURES as u8)
            .map(|i| AttestorSignature {
                pubkey: sample_pubkey(i),
                sig: SafeVec::try_from(vec![0u8; 64]).unwrap(),
            })
            .collect();
        let result = submit_attestation::<S>(schema_id, payload_hash, too_many);
        match result {
            Err(ClientError::AttestationSignaturesOverCap { actual, max }) => {
                assert_eq!(actual, MAX_ATTESTATION_SIGNATURES + 1);
                assert_eq!(max, MAX_ATTESTATION_SIGNATURES);
            }
            other => panic!("expected AttestationSignaturesOverCap, got {other:?}"),
        }
    }

    #[test]
    fn register_schema_rejects_over_cap_name() {
        // SafeString's default cap is 256 bytes; the SDK enforces
        // it via TryFrom<String>. Anything longer than that surfaces
        // as `SchemaNameOverCap`. We use 1024 chars to be well past
        // any plausible bump.
        let too_long = "a".repeat(1024);
        let attestor_set_id = AttestorSetId::from([1u8; 32]);
        let result = register_schema::<S>(too_long.clone(), 1, attestor_set_id, 0, None);
        match result {
            Err(ClientError::SchemaNameOverCap(name)) => {
                assert_eq!(name, too_long);
            }
            other => panic!("expected SchemaNameOverCap, got {other:?}"),
        }
    }

    #[test]
    fn register_schema_builds_correct_variant() {
        let attestor_set_id = AttestorSetId::from([7u8; 32]);
        let fee_addr = sample_address(9);
        let msg: CallMessage<S> = register_schema::<S>(
            "themisra.proof-of-prompt".to_string(),
            3,
            attestor_set_id,
            2500,
            Some(fee_addr),
        )
        .expect("name under cap");
        match msg {
            CallMessage::RegisterSchema {
                name,
                version,
                attestor_set,
                fee_routing_bps,
                fee_routing_addr,
            } => {
                assert_eq!(<SafeString as AsRef<str>>::as_ref(&name), "themisra.proof-of-prompt");
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
        let schema_id = sample_schema_id(1);
        let payload_hash = sample_payload_hash(2);
        let sig_bytes = vec![0xab; 64];
        let signatures = vec![AttestorSignature {
            pubkey: sample_pubkey(1),
            sig: SafeVec::try_from(sig_bytes).unwrap(),
        }];
        let msg: CallMessage<S> =
            submit_attestation::<S>(schema_id, payload_hash, signatures).expect("under cap");
        match msg {
            CallMessage::SubmitAttestation {
                schema_id: got_schema_id,
                payload_hash: got_payload_hash,
                signatures: got_signatures,
            } => {
                assert_eq!(got_schema_id, schema_id);
                assert_eq!(got_payload_hash, payload_hash);
                assert_eq!(got_signatures.len(), 1);
            }
            other => panic!("expected SubmitAttestation, got {other:?}"),
        }
    }

    // ------ Digest helper -------------------------------------------------

    #[test]
    fn attestation_digest_matches_struct_digest() {
        let schema_id = sample_schema_id(1);
        let payload_hash = sample_payload_hash(2);
        let submitter = sample_address(3);
        let timestamp = 1_700_000_000u64;

        let via_helper = attestation_digest::<S>(schema_id, payload_hash, submitter, timestamp);
        let via_struct =
            SignedAttestationPayload::<S> { schema_id, payload_hash, submitter, timestamp }
                .digest();

        assert_eq!(via_helper, via_struct);
    }

    #[test]
    fn attestation_digest_is_field_sensitive() {
        let base = attestation_digest::<S>(
            sample_schema_id(1),
            sample_payload_hash(2),
            sample_address(3),
            1_000,
        );
        // Any single-field change must yield a different digest.
        assert_ne!(
            base,
            attestation_digest::<S>(
                sample_schema_id(9),
                sample_payload_hash(2),
                sample_address(3),
                1_000,
            )
        );
        assert_ne!(
            base,
            attestation_digest::<S>(
                sample_schema_id(1),
                sample_payload_hash(9),
                sample_address(3),
                1_000,
            )
        );
        assert_ne!(
            base,
            attestation_digest::<S>(
                sample_schema_id(1),
                sample_payload_hash(2),
                sample_address(9),
                1_000,
            )
        );
        assert_ne!(
            base,
            attestation_digest::<S>(
                sample_schema_id(1),
                sample_payload_hash(2),
                sample_address(3),
                1_001,
            )
        );
    }

    // ------ sign_attestation ----------------------------------------------

    #[test]
    fn sign_attestation_produces_verifiable_signature() {
        let signer = sample_signer(7);
        let digest = attestation_digest::<S>(
            sample_schema_id(1),
            sample_payload_hash(2),
            sample_address(3),
            42,
        );

        let sig = sign_attestation(&signer, &digest);

        // pubkey matches the key's verifying-key bytes.
        assert_eq!(sig.pubkey, PubKey::from(signer.verifying_key().to_bytes()));
        // signature is the ed25519 expected length (64 bytes).
        assert_eq!(sig.sig.len(), 64);

        // And it verifies.
        let vk = VerifyingKey::from_bytes(sig.pubkey.as_bytes()).expect("pubkey valid ed25519");
        let sig_bytes: [u8; 64] = (&*sig.sig).try_into().expect("signature 64 bytes");
        let signature = ed25519_dalek::Signature::from_bytes(&sig_bytes);
        vk.verify(&digest, &signature).expect("signature verifies");
    }

    #[test]
    fn sign_attestation_rejects_wrong_digest_on_verify() {
        // Sanity: the signature binds to the digest. Flipping a byte
        // in the digest after signing breaks verification.
        let signer = sample_signer(7);
        let digest = attestation_digest::<S>(
            sample_schema_id(1),
            sample_payload_hash(2),
            sample_address(3),
            42,
        );
        let sig = sign_attestation(&signer, &digest);

        let mut tampered = digest;
        tampered[0] ^= 0x01;

        let vk = VerifyingKey::from_bytes(sig.pubkey.as_bytes()).unwrap();
        let sig_bytes: [u8; 64] = (&*sig.sig).try_into().unwrap();
        let signature = ed25519_dalek::Signature::from_bytes(&sig_bytes);
        assert!(vk.verify(&tampered, &signature).is_err());
    }

    // ------ build_signed_submit -------------------------------------------

    #[test]
    fn build_signed_submit_produces_valid_two_of_n_submission() {
        let signer_a = sample_signer(1);
        let signer_b = sample_signer(2);
        let keys: Vec<&SigningKey> = vec![&signer_a, &signer_b];

        let schema_id = sample_schema_id(0xAB);
        let payload_hash = sample_payload_hash(0xCD);
        let submitter = sample_address(5);
        let timestamp = 1_234_567_890u64;

        let msg: CallMessage<S> =
            build_signed_submit::<S>(&keys, schema_id, payload_hash, submitter, timestamp)
                .expect("under cap");

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
        let digest = attestation_digest::<S>(schema_id, payload_hash, submitter, timestamp);
        for sig in &signatures {
            let vk = VerifyingKey::from_bytes(sig.pubkey.as_bytes()).expect("valid pubkey");
            let sig_bytes: [u8; 64] = (&*sig.sig).try_into().expect("64-byte signature");
            let signature = ed25519_dalek::Signature::from_bytes(&sig_bytes);
            vk.verify(&digest, &signature).expect("signature verifies");
        }

        // And the two signatures come from the two distinct signing keys.
        let sigs: Vec<AttestorSignature> = signatures.into_iter().collect();
        assert_eq!(sigs.len(), 2);
        assert_eq!(sigs[0].pubkey, PubKey::from(signer_a.verifying_key().to_bytes()));
        assert_eq!(sigs[1].pubkey, PubKey::from(signer_b.verifying_key().to_bytes()));
    }

    #[test]
    fn build_signed_submit_with_zero_keys_is_empty_submission() {
        // Edge case: the builder doesn't enforce quorum. Zero keys
        // produces an empty signatures vector; the on-chain state
        // transition will reject below-threshold submissions.
        let keys: Vec<&SigningKey> = Vec::new();
        let msg: CallMessage<S> = build_signed_submit::<S>(
            &keys,
            sample_schema_id(1),
            sample_payload_hash(2),
            sample_address(3),
            0,
        )
        .expect("under cap");
        match msg {
            CallMessage::SubmitAttestation { signatures, .. } => {
                assert_eq!(signatures.len(), 0);
            }
            other => panic!("expected SubmitAttestation, got {other:?}"),
        }
    }

    // ------ Re-exports smoke test -----------------------------------------

    #[test]
    fn re_exported_schema_derive_id_matches_upstream() {
        // The re-exports are the same types as in the `attestation` crate.
        let owner = sample_address(1);
        let id = Schema::<S>::derive_id(&owner, "foo", 1);
        let upstream = attestation::Schema::<S>::derive_id(&owner, "foo", 1);
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
