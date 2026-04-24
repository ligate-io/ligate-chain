//! Ligate attestation protocol, generic on-chain attestation primitive.
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
//! are exposed via query RPC and deliberately absent from [`CallMessage`] ,
//! verification is a read, not a state-mutating transaction.
//!
//! # Scope of this crate
//!
//! Protocol-shaped data types + the [`AttestationModule`] struct with its
//! state layout wired to the real `sov_modules_api::StateMap`. State
//! transition implementations (`call`), genesis config, RPC endpoints,
//! and fee-split enforcement land in sibling issues (#7, #8, #20-#22).

#![deny(missing_docs)]

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sov_modules_api::{
    prelude::*, CallResponse, Context, Error, Module, ModuleInfo, StateMap, WorkingSet,
};
use thiserror::Error as ThisError;

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

/// A quorum of signers. **Immutable once registered**, to "rotate" an
/// attestor set, register a new one and bump any affected schema to a new
/// version that points at the new [`AttestorSetId`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct AttestorSet {
    /// Ordered list of attestor public keys.
    ///
    /// Order matters for the derivation of [`AttestorSetId`], callers
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
/// Registration is permissionless, anyone with enough $LGT to pay the
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
    /// `fee_routing_bps == 0`, the state transition rejects dangling
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
/// Each pair is write-once, resubmitting the same pair is rejected by
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
        let bytes = self.try_to_vec().expect("Borsh serialization of owned data is infallible");
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

// ----- Module struct ---------------------------------------------------------

/// The Ligate attestation module.
///
/// Three state fields map 1-to-1 to the three protocol entities:
///
/// - [`AttestationModule::schemas`], `SchemaId → Schema`
/// - [`AttestationModule::attestor_sets`], `AttestorSetId → AttestorSet`
/// - [`AttestationModule::attestations`], `(SchemaId, PayloadHash) → Attestation`
///
/// Call handlers and genesis config live in sibling issues; this struct
/// exists to lock down the state layout that every other piece of the
/// module keys off.
#[derive(Clone, ModuleInfo)]
pub struct AttestationModule<C: Context> {
    /// Module's own on-chain address. Filled by the runtime.
    #[address]
    pub address: C::Address,

    /// Schema registry: `SchemaId → Schema`.
    #[state]
    pub schemas: StateMap<SchemaId, Schema>,

    /// AttestorSet registry: `AttestorSetId → AttestorSet`.
    #[state]
    pub attestor_sets: StateMap<AttestorSetId, AttestorSet>,

    /// Attestation store: `(SchemaId, PayloadHash) → Attestation`.
    #[state]
    pub attestations: StateMap<AttestationKey, Attestation>,
}

// ----- Protocol constants -----------------------------------------------------

/// Maximum `fee_routing_bps` accepted at schema registration (5000 = 50%).
/// Governance-adjustable in v1+; hardcoded for v0.
pub const MAX_BUILDER_BPS: u16 = 5000;

/// Default attestation fee in $LGT, in the chain's smallest unit.
/// Placeholder constant. Real fee sizing is a governance parameter set
/// at launch. Documented here for reference against the spec.
pub const DEFAULT_ATTESTATION_FEE_LGT_MICROS: u64 = 1_000; // 0.001 $LGT at 6 decimals

// ============================================================================
// Errors
// ============================================================================

/// Typed errors produced by the attestation module's call handlers.
///
/// These convert into [`sov_modules_api::Error`] (via [`anyhow::Error`]) when
/// returned from a Module handler, so callers of the RPC can pattern-match
/// on the wire shape while the chain runtime stays generic.
#[derive(Debug, ThisError)]
pub enum AttestationError {
    /// `threshold` was greater than the number of supplied members.
    #[error("attestor set threshold ({threshold}) exceeds member count ({count})")]
    ThresholdExceedsMembers {
        /// Supplied threshold.
        threshold: u8,
        /// Number of members supplied.
        count: usize,
    },

    /// A threshold of 0 was supplied, which would accept attestations with no
    /// signatures.
    #[error("attestor set threshold must be at least 1")]
    ZeroThreshold,

    /// Members list was empty.
    #[error("attestor set must have at least one member")]
    EmptyAttestorSet,

    /// An attestor set with this derived id already exists.
    #[error("attestor set already registered")]
    DuplicateAttestorSet,

    /// A schema with this derived id already exists (same owner, name, version).
    #[error("schema already registered")]
    DuplicateSchema,

    /// Schema registration referenced an [`AttestorSetId`] that isn't stored.
    #[error("schema references an unregistered attestor set")]
    UnknownAttestorSet,

    /// `fee_routing_bps` exceeded the protocol cap [`MAX_BUILDER_BPS`].
    #[error("fee routing bps ({bps}) exceeds protocol cap ({cap})")]
    FeeRoutingExceedsCap {
        /// Supplied basis points.
        bps: u16,
        /// Protocol cap (v0: 5000 = 50%).
        cap: u16,
    },

    /// Fee routing was misconfigured: address set without basis points, or
    /// basis points > 0 without an address.
    #[error(
        "fee routing misconfigured: address and basis points must be set together or both absent"
    )]
    OrphanFeeRouting,

    /// Submission referenced a [`SchemaId`] that isn't stored.
    #[error("schema not found")]
    UnknownSchema,

    /// An attestation with this `(schema_id, payload_hash)` already exists.
    #[error("attestation already exists for this (schema_id, payload_hash)")]
    DuplicateAttestation,

    /// `context.sender()` did not produce a 32-byte representation.
    #[error("sender address is not 32 bytes")]
    BadSenderLength,
}

// ============================================================================
// Module implementation
// ============================================================================

impl<C: Context> Module for AttestationModule<C> {
    type Context = C;
    type Config = ();
    type CallMessage = CallMessage;
    type Event = ();

    fn genesis(
        &self,
        _config: &Self::Config,
        _working_set: &mut WorkingSet<C>,
    ) -> Result<(), Error> {
        // v0: no pre-seeded state. Schemas and attestor sets are registered
        // via tx after launch. Genesis-seeding (e.g. for the Themisra attestor
        // set) lands with #8.
        Ok(())
    }

    fn call(
        &self,
        msg: Self::CallMessage,
        context: &Self::Context,
        working_set: &mut WorkingSet<C>,
    ) -> Result<CallResponse, Error> {
        match msg {
            CallMessage::RegisterAttestorSet { members, threshold } => {
                Ok(self.handle_register_attestor_set(members, threshold, working_set)?)
            }
            CallMessage::RegisterSchema {
                name,
                version,
                attestor_set,
                fee_routing_bps,
                fee_routing_addr,
            } => Ok(self.handle_register_schema(
                name,
                version,
                attestor_set,
                fee_routing_bps,
                fee_routing_addr,
                context,
                working_set,
            )?),
            CallMessage::SubmitAttestation { schema_id, payload_hash, signatures } => Ok(self
                .handle_submit_attestation(
                    schema_id,
                    payload_hash,
                    signatures,
                    context,
                    working_set,
                )?),
        }
    }
}

impl<C: Context> AttestationModule<C> {
    /// Handle [`CallMessage::RegisterAttestorSet`].
    fn handle_register_attestor_set(
        &self,
        members: Vec<PubKey>,
        threshold: u8,
        working_set: &mut WorkingSet<C>,
    ) -> anyhow::Result<CallResponse> {
        if members.is_empty() {
            return Err(AttestationError::EmptyAttestorSet.into());
        }
        if threshold == 0 {
            return Err(AttestationError::ZeroThreshold.into());
        }
        if usize::from(threshold) > members.len() {
            return Err(AttestationError::ThresholdExceedsMembers {
                threshold,
                count: members.len(),
            }
            .into());
        }

        // Store members in sorted order so later reads match the id derivation.
        let mut sorted_members = members;
        sorted_members.sort_unstable();

        let id = AttestorSet::derive_id(&sorted_members, threshold);
        if self.attestor_sets.get(&id, working_set).is_some() {
            return Err(AttestationError::DuplicateAttestorSet.into());
        }

        self.attestor_sets.set(
            &id,
            &AttestorSet { members: sorted_members, threshold },
            working_set,
        );

        Ok(CallResponse::default())
    }

    /// Handle [`CallMessage::RegisterSchema`].
    #[allow(clippy::too_many_arguments)]
    fn handle_register_schema(
        &self,
        name: String,
        version: u32,
        attestor_set: AttestorSetId,
        fee_routing_bps: u16,
        fee_routing_addr: Option<Address>,
        context: &C,
        working_set: &mut WorkingSet<C>,
    ) -> anyhow::Result<CallResponse> {
        if fee_routing_bps > MAX_BUILDER_BPS {
            return Err(AttestationError::FeeRoutingExceedsCap {
                bps: fee_routing_bps,
                cap: MAX_BUILDER_BPS,
            }
            .into());
        }
        if (fee_routing_bps > 0) != fee_routing_addr.is_some() {
            return Err(AttestationError::OrphanFeeRouting.into());
        }
        if self.attestor_sets.get(&attestor_set, working_set).is_none() {
            return Err(AttestationError::UnknownAttestorSet.into());
        }

        let owner = sender_to_address::<C>(context.sender())?;
        let id = Schema::derive_id(&owner, &name, version);

        if self.schemas.get(&id, working_set).is_some() {
            return Err(AttestationError::DuplicateSchema.into());
        }

        self.schemas.set(
            &id,
            &Schema { owner, name, version, attestor_set, fee_routing_bps, fee_routing_addr },
            working_set,
        );

        Ok(CallResponse::default())
    }

    /// Handle [`CallMessage::SubmitAttestation`].
    ///
    /// Signature validation is **stubbed** in this revision (issue #20 owns
    /// the real ed25519 verification + quorum enforcement). For now we just
    /// require the schema and the signatures vector to be structurally
    /// well-formed.
    fn handle_submit_attestation(
        &self,
        schema_id: SchemaId,
        payload_hash: Hash32,
        signatures: Vec<AttestorSignature>,
        context: &C,
        working_set: &mut WorkingSet<C>,
    ) -> anyhow::Result<CallResponse> {
        if self.schemas.get(&schema_id, working_set).is_none() {
            return Err(AttestationError::UnknownSchema.into());
        }

        let key = (schema_id, payload_hash);
        if self.attestations.get(&key, working_set).is_some() {
            return Err(AttestationError::DuplicateAttestation.into());
        }

        // TODO(#20): verify each `AttestorSignature` against the schema's
        // `AttestorSet`, check quorum threshold, enforce no-duplicate-pubkeys.
        // TODO(#22): charge `ATTESTATION_FEE` via bank and split per
        // schema.fee_routing_bps / schema.fee_routing_addr.

        let submitter = sender_to_address::<C>(context.sender())?;
        // TODO: pull the real block timestamp once WorkingSet exposes it
        // (slot/header access lives behind the rollup-blueprint layer and is
        // wired in #13). For now, attestations carry `timestamp = 0`; the
        // write-once key prevents replay either way.
        let timestamp = 0u64;

        self.attestations.set(
            &key,
            &Attestation { schema_id, payload_hash, submitter, timestamp, signatures },
            working_set,
        );

        Ok(CallResponse::default())
    }
}

/// Convert the runtime's `C::Address` to the protocol's fixed 32-byte
/// [`Address`] shape.
///
/// Relies on `C::Address` implementing `AsRef<[u8]>` (standard for Sovereign
/// SDK default contexts). Rejects anything that doesn't hand back exactly 32
/// bytes, so the protocol's on-chain representation stays unambiguous even
/// if a Context with a non-32-byte address is introduced later.
fn sender_to_address<C: Context>(sender: &C::Address) -> anyhow::Result<Address> {
    let bytes: &[u8] = sender.as_ref();
    if bytes.len() != 32 {
        return Err(AttestationError::BadSenderLength.into());
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(bytes);
    Ok(out)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip_borsh<T>(value: T)
    where
        T: BorshSerialize + BorshDeserialize + PartialEq + std::fmt::Debug,
    {
        let bytes = value.try_to_vec().expect("encode");
        let decoded = T::try_from_slice(&bytes).expect("decode");
        assert_eq!(value, decoded);
    }

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
    fn call_message_borsh_round_trip() {
        for msg in [
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
        ] {
            round_trip_borsh(msg);
        }
    }

    #[test]
    fn signed_payload_borsh_round_trip() {
        round_trip_borsh(SignedAttestationPayload {
            schema_id: [9u8; 32],
            payload_hash: [1u8; 32],
            submitter: sample_address(7),
            timestamp: 1_234_567_890,
        });
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

// ============================================================================
// State-transition tests (end-to-end via module.call)
// ============================================================================

#[cfg(test)]
mod state_transition_tests {
    use sov_modules_api::default_context::DefaultContext;
    use sov_modules_api::{Address as SovAddress, Module as _, WorkingSet};
    use sov_prover_storage_manager::new_orphan_storage;

    use super::*;

    type TestModule = AttestationModule<DefaultContext>;

    fn fresh() -> (TestModule, DefaultContext, WorkingSet<DefaultContext>, tempfile::TempDir) {
        let tmpdir = tempfile::tempdir().unwrap();
        let working_set = WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
        let sender = SovAddress::from([1u8; 32]);
        let sequencer = SovAddress::from([9u8; 32]);
        let context = DefaultContext::new(sender, sequencer, 1);
        let module = TestModule::default();
        (module, context, working_set, tmpdir)
    }

    fn sample_members() -> Vec<PubKey> {
        vec![[1u8; 32], [2u8; 32], [3u8; 32]]
    }

    #[test]
    fn register_attestor_set_happy_path() {
        let (module, context, mut ws, _td) = fresh();

        let res = module.call(
            CallMessage::RegisterAttestorSet { members: sample_members(), threshold: 2 },
            &context,
            &mut ws,
        );
        assert!(res.is_ok(), "register should succeed: {res:?}");

        // Verify it's in state
        let id = AttestorSet::derive_id(&sample_members(), 2);
        let stored = module.attestor_sets.get(&id, &mut ws);
        assert!(stored.is_some(), "attestor set should be stored");
        let stored = stored.unwrap();
        assert_eq!(stored.threshold, 2);
        assert_eq!(stored.members.len(), 3);
    }

    #[test]
    fn register_attestor_set_rejects_threshold_over_members() {
        let (module, context, mut ws, _td) = fresh();
        let res = module.call(
            CallMessage::RegisterAttestorSet { members: vec![[1u8; 32]], threshold: 2 },
            &context,
            &mut ws,
        );
        assert!(res.is_err());
    }

    #[test]
    fn register_attestor_set_rejects_zero_threshold() {
        let (module, context, mut ws, _td) = fresh();
        let res = module.call(
            CallMessage::RegisterAttestorSet { members: sample_members(), threshold: 0 },
            &context,
            &mut ws,
        );
        assert!(res.is_err());
    }

    #[test]
    fn register_schema_requires_existing_attestor_set() {
        let (module, context, mut ws, _td) = fresh();
        let res = module.call(
            CallMessage::RegisterSchema {
                name: "foo".into(),
                version: 1,
                attestor_set: [0xAAu8; 32],
                fee_routing_bps: 0,
                fee_routing_addr: None,
            },
            &context,
            &mut ws,
        );
        assert!(res.is_err(), "expected UnknownAttestorSet");
    }

    #[test]
    fn register_schema_rejects_over_cap_fee_routing() {
        let (module, context, mut ws, _td) = fresh();
        // Seed the attestor set first so we isolate the fee-routing check
        module
            .call(
                CallMessage::RegisterAttestorSet { members: sample_members(), threshold: 2 },
                &context,
                &mut ws,
            )
            .unwrap();
        let attestor_set_id = AttestorSet::derive_id(&sample_members(), 2);

        let res = module.call(
            CallMessage::RegisterSchema {
                name: "foo".into(),
                version: 1,
                attestor_set: attestor_set_id,
                fee_routing_bps: 6000, // > MAX_BUILDER_BPS (5000)
                fee_routing_addr: Some([2u8; 32]),
            },
            &context,
            &mut ws,
        );
        assert!(res.is_err(), "expected FeeRoutingExceedsCap");
    }

    #[test]
    fn register_schema_rejects_orphan_routing() {
        let (module, context, mut ws, _td) = fresh();
        module
            .call(
                CallMessage::RegisterAttestorSet { members: sample_members(), threshold: 2 },
                &context,
                &mut ws,
            )
            .unwrap();
        let attestor_set_id = AttestorSet::derive_id(&sample_members(), 2);

        // bps > 0 but no addr
        let res = module.call(
            CallMessage::RegisterSchema {
                name: "foo".into(),
                version: 1,
                attestor_set: attestor_set_id,
                fee_routing_bps: 1000,
                fee_routing_addr: None,
            },
            &context,
            &mut ws,
        );
        assert!(res.is_err());

        // addr set but bps == 0
        let res = module.call(
            CallMessage::RegisterSchema {
                name: "foo".into(),
                version: 1,
                attestor_set: attestor_set_id,
                fee_routing_bps: 0,
                fee_routing_addr: Some([2u8; 32]),
            },
            &context,
            &mut ws,
        );
        assert!(res.is_err());
    }

    #[test]
    fn submit_attestation_requires_existing_schema() {
        let (module, context, mut ws, _td) = fresh();
        let res = module.call(
            CallMessage::SubmitAttestation {
                schema_id: [0xDEu8; 32],
                payload_hash: [0xADu8; 32],
                signatures: vec![],
            },
            &context,
            &mut ws,
        );
        assert!(res.is_err(), "expected UnknownSchema");
    }

    #[test]
    fn full_register_register_submit_flow() {
        let (module, context, mut ws, _td) = fresh();

        // 1. Register attestor set
        module
            .call(
                CallMessage::RegisterAttestorSet { members: sample_members(), threshold: 2 },
                &context,
                &mut ws,
            )
            .expect("register attestor set");
        let attestor_set_id = AttestorSet::derive_id(&sample_members(), 2);

        // 2. Register schema
        module
            .call(
                CallMessage::RegisterSchema {
                    name: "themisra.proof-of-prompt".into(),
                    version: 1,
                    attestor_set: attestor_set_id,
                    fee_routing_bps: 0,
                    fee_routing_addr: None,
                },
                &context,
                &mut ws,
            )
            .expect("register schema");
        let owner = [1u8; 32]; // matches the sender in fresh()
        let schema_id = Schema::derive_id(&owner, "themisra.proof-of-prompt", 1);

        // 3. Submit attestation
        let payload_hash = [0x42u8; 32];
        module
            .call(
                CallMessage::SubmitAttestation {
                    schema_id,
                    payload_hash,
                    signatures: vec![AttestorSignature {
                        pubkey: [1u8; 32],
                        sig: vec![0xAAu8; 64],
                    }],
                },
                &context,
                &mut ws,
            )
            .expect("submit attestation");

        // 4. Verify state
        let stored = module.attestations.get(&(schema_id, payload_hash), &mut ws);
        assert!(stored.is_some(), "attestation should be stored");
        let stored = stored.unwrap();
        assert_eq!(stored.schema_id, schema_id);
        assert_eq!(stored.payload_hash, payload_hash);
        assert_eq!(stored.submitter, owner);

        // 5. Write-once: resubmitting same (schema_id, payload_hash) must fail
        let res = module.call(
            CallMessage::SubmitAttestation { schema_id, payload_hash, signatures: vec![] },
            &context,
            &mut ws,
        );
        assert!(res.is_err(), "duplicate (schema_id, payload_hash) must be rejected");
    }
}
