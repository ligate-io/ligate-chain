//! Ligate attestation protocol — generic on-chain attestation primitive.
//!
//! This crate implements the protocol's data shapes and state layout.
//! See [`docs/protocol/attestation-v0.md`][spec] for the full specification.
//!
//! [spec]: https://github.com/ligate-io/ligate-chain/blob/main/docs/protocol/attestation-v0.md
//!
//! # Protocol summary
//!
//! Three entities, three `StateMap`s, three call messages:
//!
//! | Entity | Identified by | Registered by |
//! |---|---|---|
//! | [`AttestorSet`]  | `SHA-256(members ‖ threshold)` | [`CallMessage::RegisterAttestorSet`] |
//! | [`Schema`]       | `SHA-256(owner ‖ name ‖ version)` | [`CallMessage::RegisterSchema`] |
//! | [`Attestation`]  | `(schema_id, payload_hash)` | [`CallMessage::SubmitAttestation`] |
//!
//! Reads (`get_schema`, `get_attestor_set`, `get_attestation`, listing variants)
//! are exposed via query RPC and deliberately absent from [`CallMessage`] —
//! verification is a read, not a state-mutating transaction.
//!
//! # Scope of this crate
//!
//! **Types + state layout only.** State transitions (`call`), genesis,
//! query RPC, and fee-split enforcement land in sibling issues once the
//! Sovereign SDK integration is unblocked. The receipt store is aliased
//! to [`BTreeMap`] as a stand-in for `sov_modules_api::StateMap` so
//! downstream crates (Themisra's schema crate, the client SDK, unit
//! tests) can name the type against a concrete shape.

#![deny(missing_docs)]

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

// ----- Primitive aliases ------------------------------------------------------

/// 32-byte SHA-256 digest used for every hash field on the protocol.
pub type Hash32 = [u8; 32];

/// Canonical 32-byte address type. The real chain-level address type will
/// be an `S::Address` once the Sovereign SDK is wired in; the raw-bytes
/// alias here lets us carry the same shape without the generic bound.
pub type Address = Hash32;

/// Public key material. Ed25519 for v0 (32 bytes); the placeholder keeps
/// the protocol shape flexible for later aggregate schemes (BLS, etc.).
pub type PubKey = Hash32;

/// Schema identifier: `SHA-256(owner ‖ name ‖ version)`.
pub type SchemaId = Hash32;

/// AttestorSet identifier: `SHA-256(members ‖ threshold)`.
pub type AttestorSetId = Hash32;

// ----- AttestorSet ------------------------------------------------------------

/// A quorum of signers. **Immutable once registered** — to "rotate" an
/// attestor set, register a new one and bump any affected schema to a new
/// version that points at the new [`AttestorSetId`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct AttestorSet {
    /// Ordered list of attestor public keys.
    ///
    /// Order matters for the derivation of [`AttestorSetId`] — callers
    /// must supply a canonical (e.g. lexicographic) ordering, and the
    /// state transition re-hashes to verify before accepting.
    pub members: Vec<PubKey>,
    /// M-of-N signatures required. `threshold ≤ members.len()` is an
    /// invariant the state transition enforces at registration time.
    pub threshold: u8,
}

/// Single attestor's co-signature on an [`Attestation`].
///
/// `sig` is `Vec<u8>` rather than a fixed-size array so we can support
/// different signature schemes (ed25519 = 64 bytes, BLS = 48, etc.) under
/// the same type; exact validation is scheme-specific and lives in the
/// state transition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct AttestorSignature {
    /// Signer's public key. Must appear in the schema's
    /// [`AttestorSet::members`] for the signature to be accepted.
    pub pubkey: PubKey,
    /// Signature over the canonical Borsh encoding of
    /// [`SignedAttestationPayload`].
    pub sig: Vec<u8>,
}

// ----- Schema -----------------------------------------------------------------

/// An app's attestation shape and rules.
///
/// Registration is permissionless — anyone with enough $LGT to pay the
/// registration fee can register a schema. Name collisions across owners
/// are allowed (different `owner` bytes → different [`SchemaId`]); social
/// identity of the "real" schema owner lives off-chain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Schema {
    /// Registrant; receives control over schema version upgrades and
    /// [`Schema::fee_routing_addr`]. Schema fields are otherwise immutable.
    pub owner: Address,
    /// Human-readable handle, e.g. `"themisra.proof-of-prompt"`. Only
    /// unique within an `owner`'s namespace.
    pub name: String,
    /// Monotonic per-schema version. Bumping version = new
    /// [`SchemaId`] = new [`Schema`] entry; no in-place mutation.
    pub version: u32,
    /// [`AttestorSetId`] of the quorum that must sign attestations under
    /// this schema. Must reference a registered [`AttestorSet`].
    pub attestor_set: AttestorSetId,
    /// Share of the attestation fee routed to [`Schema::fee_routing_addr`],
    /// in basis points (1/10,000). Protocol caps this at 5000 (50%) in
    /// v0; governance-adjustable in v1+.
    pub fee_routing_bps: u16,
    /// Destination of the builder's fee share. `None` iff
    /// `fee_routing_bps == 0` — the state transition rejects dangling
    /// routing and orphan addresses at registration time.
    pub fee_routing_addr: Option<Address>,
}

impl Schema {
    /// Derive a [`SchemaId`] from its defining tuple.
    ///
    /// Formula: `SHA-256(owner ‖ name ‖ version_le_bytes)`. The
    /// state-transition re-runs this at registration and stores the
    /// derived id on-chain, so downstream consumers never have to trust
    /// a submitter-claimed id.
    pub fn derive_id(owner: &Address, name: &str, version: u32) -> SchemaId {
        let mut hasher = Sha256::new();
        hasher.update(owner);
        hasher.update(name.as_bytes());
        hasher.update(version.to_le_bytes());
        hasher.finalize().into()
    }
}

impl AttestorSet {
    /// Derive an [`AttestorSetId`] from members + threshold.
    ///
    /// Members are **sorted internally** before hashing so the set id is
    /// independent of the caller's input order. The state transition
    /// stores members in the same sorted order it uses for derivation.
    /// Duplicate pubkeys are preserved (the state transition rejects
    /// sets with duplicates at registration).
    pub fn derive_id(members: &[PubKey], threshold: u8) -> AttestorSetId {
        let mut sorted = members.to_vec();
        sorted.sort_unstable();
        let mut hasher = Sha256::new();
        for m in &sorted {
            hasher.update(m);
        }
        hasher.update([threshold]);
        hasher.finalize().into()
    }
}

// ----- Attestation ------------------------------------------------------------

/// Key used for the attestation store: `(schema_id, payload_hash)`.
///
/// Each pair is write-once — resubmitting the same pair is rejected by
/// the state transition. This is also what provides replay protection for
/// the attestor signatures.
pub type AttestationKey = (SchemaId, Hash32);

/// Stored attestation record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Attestation {
    /// Schema this attestation is bound to. Dictates payload semantics
    /// and which attestor set must have signed.
    pub schema_id: SchemaId,
    /// Caller-computed digest of the off-chain payload. The chain never
    /// sees the plaintext.
    pub payload_hash: Hash32,
    /// Address that submitted the attestation. Paid the fee; owns the
    /// record for accounting purposes.
    pub submitter: Address,
    /// Unix seconds, sourced from the transaction's block header (not
    /// submitter-provided, to prevent timestamp games).
    pub timestamp: u64,
    /// Quorum signatures over [`SignedAttestationPayload`]. All pubkeys
    /// must be ∈ `schema.attestor_set.members`; count of valid sigs
    /// must be ≥ `schema.attestor_set.threshold`.
    pub signatures: Vec<AttestorSignature>,
}

/// Canonical payload attestors sign over.
///
/// Borsh-encoded, hashed, and signed off-chain by each attestor before
/// the submitter aggregates signatures and submits to the chain.
///
/// Deliberately excludes the [`Attestation::signatures`] field (circular)
/// and any caller-adversarial data. Replay protection comes from the
/// write-once `(schema_id, payload_hash)` invariant, not from this struct.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct SignedAttestationPayload {
    /// See [`Attestation::schema_id`].
    pub schema_id: SchemaId,
    /// See [`Attestation::payload_hash`].
    pub payload_hash: Hash32,
    /// See [`Attestation::submitter`].
    pub submitter: Address,
    /// See [`Attestation::timestamp`].
    pub timestamp: u64,
}

impl SignedAttestationPayload {
    /// SHA-256 of the canonical Borsh encoding of this payload.
    ///
    /// This is the digest each attestor signs over. Exposed as a
    /// library helper so attestor quorum software and SDKs don't have
    /// to re-implement the encoding rule.
    pub fn digest(&self) -> Hash32 {
        let bytes = borsh::to_vec(self).expect("Borsh serialization of owned data is infallible");
        Sha256::digest(&bytes).into()
    }
}

// ----- Messages ---------------------------------------------------------------

/// State-changing messages accepted by the attestation module.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub enum CallMessage {
    /// Register a new [`AttestorSet`].
    ///
    /// Costs `ATTESTOR_SET_FEE` $LGT (governance, default 10 $LGT).
    /// Derivation: `SHA-256(borsh(members) ‖ threshold)`. Rejected if a
    /// set with the derived id already exists.
    RegisterAttestorSet {
        /// See [`AttestorSet::members`].
        members: Vec<PubKey>,
        /// See [`AttestorSet::threshold`].
        threshold: u8,
    },

    /// Register a new [`Schema`].
    ///
    /// Costs `SCHEMA_REGISTRATION_FEE` $LGT (governance, default 100 $LGT).
    /// Owner is the transaction submitter. `attestor_set` must reference
    /// an already-registered [`AttestorSet`]. `fee_routing_bps` must be
    /// ≤ `MAX_BUILDER_BPS` (5000 in v0). `fee_routing_addr` must be
    /// `Some` iff `fee_routing_bps > 0`.
    RegisterSchema {
        /// See [`Schema::name`].
        name: String,
        /// See [`Schema::version`].
        version: u32,
        /// See [`Schema::attestor_set`].
        attestor_set: AttestorSetId,
        /// See [`Schema::fee_routing_bps`].
        fee_routing_bps: u16,
        /// See [`Schema::fee_routing_addr`].
        fee_routing_addr: Option<Address>,
    },

    /// Submit a new [`Attestation`] under an existing schema.
    ///
    /// Costs `ATTESTATION_FEE` $LGT (governance, default 0.001 $LGT),
    /// split per [`Schema::fee_routing_bps`]. Signatures are validated
    /// against the schema's [`AttestorSet`] at submission time.
    SubmitAttestation {
        /// See [`Attestation::schema_id`].
        schema_id: SchemaId,
        /// See [`Attestation::payload_hash`].
        payload_hash: Hash32,
        /// See [`Attestation::signatures`].
        signatures: Vec<AttestorSignature>,
    },
}

// ----- State layout (placeholder) --------------------------------------------

/// Schema store, keyed by [`SchemaId`]. Placeholder for the eventual
/// `sov_modules_api::StateMap<SchemaId, Schema>` — see crate-level docs.
pub type SchemaStore = BTreeMap<SchemaId, Schema>;

/// Attestor-set store, keyed by [`AttestorSetId`]. Placeholder for the
/// eventual `sov_modules_api::StateMap<AttestorSetId, AttestorSet>`.
pub type AttestorSetStore = BTreeMap<AttestorSetId, AttestorSet>;

/// Attestation store, keyed by [`AttestationKey`]. Placeholder for the
/// eventual `sov_modules_api::StateMap<AttestationKey, Attestation>`.
pub type AttestationStore = BTreeMap<AttestationKey, Attestation>;

// ----- Protocol constants -----------------------------------------------------

/// Maximum `fee_routing_bps` accepted at schema registration (5000 = 50%).
/// Governance-adjustable in v1+; hardcoded for v0.
pub const MAX_BUILDER_BPS: u16 = 5000;

/// Default attestation fee in $LGT, in the chain's smallest unit.
/// Placeholder constant — real fee sizing is a governance parameter set
/// at launch. Documented here for reference against the spec.
pub const DEFAULT_ATTESTATION_FEE_LGT_MICROS: u64 = 1_000; // 0.001 $LGT at 6 decimals

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_address(n: u8) -> Address {
        [n; 32]
    }

    fn sample_pubkey(n: u8) -> PubKey {
        [n; 32]
    }

    fn sample_schema() -> Schema {
        Schema {
            owner: sample_address(42),
            name: "themisra.proof-of-prompt".to_string(),
            version: 1,
            attestor_set: [7u8; 32],
            fee_routing_bps: 2500,
            fee_routing_addr: Some(sample_address(99)),
        }
    }

    fn sample_attestor_set() -> AttestorSet {
        AttestorSet {
            members: vec![sample_pubkey(1), sample_pubkey(2), sample_pubkey(3)],
            threshold: 2,
        }
    }

    fn sample_attestation() -> Attestation {
        Attestation {
            schema_id: [3u8; 32],
            payload_hash: [4u8; 32],
            submitter: sample_address(5),
            timestamp: 1_700_000_000,
            signatures: vec![AttestorSignature { pubkey: sample_pubkey(1), sig: vec![0xab; 64] }],
        }
    }

    // ------ Borsh round-trips --------------------------------------------

    #[test]
    fn schema_borsh_round_trip() {
        let original = sample_schema();
        let bytes = borsh::to_vec(&original).expect("encode");
        let decoded: Schema = borsh::from_slice(&bytes).expect("decode");
        assert_eq!(original, decoded);
    }

    #[test]
    fn attestor_set_borsh_round_trip() {
        let original = sample_attestor_set();
        let bytes = borsh::to_vec(&original).expect("encode");
        let decoded: AttestorSet = borsh::from_slice(&bytes).expect("decode");
        assert_eq!(original, decoded);
    }

    #[test]
    fn attestation_borsh_round_trip() {
        let original = sample_attestation();
        let bytes = borsh::to_vec(&original).expect("encode");
        let decoded: Attestation = borsh::from_slice(&bytes).expect("decode");
        assert_eq!(original, decoded);
    }

    #[test]
    fn call_message_borsh_round_trip() {
        let msgs = vec![
            CallMessage::RegisterAttestorSet {
                members: vec![sample_pubkey(1), sample_pubkey(2)],
                threshold: 1,
            },
            CallMessage::RegisterSchema {
                name: "foo.bar".into(),
                version: 3,
                attestor_set: [8u8; 32],
                fee_routing_bps: 1000,
                fee_routing_addr: Some(sample_address(10)),
            },
            CallMessage::SubmitAttestation {
                schema_id: [1u8; 32],
                payload_hash: [2u8; 32],
                signatures: vec![AttestorSignature {
                    pubkey: sample_pubkey(1),
                    sig: vec![0xcd; 64],
                }],
            },
        ];
        for msg in msgs {
            let bytes = borsh::to_vec(&msg).expect("encode");
            let decoded: CallMessage = borsh::from_slice(&bytes).expect("decode");
            assert_eq!(msg, decoded);
        }
    }

    #[test]
    fn signed_payload_borsh_round_trip() {
        let original = SignedAttestationPayload {
            schema_id: [9u8; 32],
            payload_hash: [1u8; 32],
            submitter: sample_address(7),
            timestamp: 1_234_567_890,
        };
        let bytes = borsh::to_vec(&original).expect("encode");
        let decoded: SignedAttestationPayload = borsh::from_slice(&bytes).expect("decode");
        assert_eq!(original, decoded);
    }

    // ------ JSON round-trips (serde) -------------------------------------

    #[test]
    fn schema_json_round_trip() {
        let original = sample_schema();
        let json = serde_json::to_string(&original).expect("encode");
        let decoded: Schema = serde_json::from_str(&json).expect("decode");
        assert_eq!(original, decoded);
    }

    #[test]
    fn attestation_json_round_trip() {
        let original = sample_attestation();
        let json = serde_json::to_string(&original).expect("encode");
        let decoded: Attestation = serde_json::from_str(&json).expect("decode");
        assert_eq!(original, decoded);
    }

    // ------ Schema::derive_id --------------------------------------------

    #[test]
    fn schema_derive_id_is_deterministic() {
        let a = Schema::derive_id(&sample_address(1), "foo", 1);
        let b = Schema::derive_id(&sample_address(1), "foo", 1);
        assert_eq!(a, b);
    }

    #[test]
    fn schema_derive_id_separates_owners() {
        // Same name + version, different owner → different schema_id.
        // This is the core anti-squatting property: namespaces are
        // owner-scoped, not global.
        let owner_a = Schema::derive_id(&sample_address(1), "themisra.pop", 1);
        let owner_b = Schema::derive_id(&sample_address(2), "themisra.pop", 1);
        assert_ne!(owner_a, owner_b);
    }

    #[test]
    fn schema_derive_id_bumps_on_version() {
        let v1 = Schema::derive_id(&sample_address(1), "foo", 1);
        let v2 = Schema::derive_id(&sample_address(1), "foo", 2);
        assert_ne!(v1, v2);
    }

    #[test]
    fn schema_derive_id_separates_names() {
        let a = Schema::derive_id(&sample_address(1), "foo", 1);
        let b = Schema::derive_id(&sample_address(1), "bar", 1);
        assert_ne!(a, b);
    }

    // ------ AttestorSet::derive_id ---------------------------------------

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

    // ------ SignedAttestationPayload::digest -----------------------------

    #[test]
    fn signed_payload_digest_is_deterministic() {
        let payload = SignedAttestationPayload {
            schema_id: [1u8; 32],
            payload_hash: [2u8; 32],
            submitter: sample_address(3),
            timestamp: 1_000_000,
        };
        assert_eq!(payload.digest(), payload.digest());
    }

    #[test]
    fn signed_payload_digest_changes_with_fields() {
        let base = SignedAttestationPayload {
            schema_id: [1u8; 32],
            payload_hash: [2u8; 32],
            submitter: sample_address(3),
            timestamp: 1_000_000,
        };
        let mut mutated = base.clone();
        mutated.timestamp += 1;
        assert_ne!(base.digest(), mutated.digest());
    }

    // ------ Protocol constants -------------------------------------------

    #[test]
    fn max_builder_bps_is_half() {
        // Documenting the invariant explicitly so a future refactor
        // doesn't silently raise the cap without thought.
        assert_eq!(MAX_BUILDER_BPS, 5000);
    }
}
