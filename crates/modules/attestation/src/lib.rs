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
