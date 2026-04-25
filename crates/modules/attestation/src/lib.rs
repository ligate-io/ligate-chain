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
    prelude::*, CallResponse, Context, Error, Module, ModuleInfo, StateMap, StateValue, WorkingSet,
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

    /// Treasury address, receives the treasury share of every attestation
    /// fee and the full amount of schema / attestor-set registration fees.
    /// Set once at genesis, governance-adjustable later.
    #[state]
    pub treasury: StateValue<Address>,

    /// Flat per-attestation fee in `$LGT` micros. Set at genesis.
    #[state]
    pub attestation_fee: StateValue<u64>,

    /// Flat schema-registration fee in `$LGT` micros. Set at genesis.
    #[state]
    pub schema_registration_fee: StateValue<u64>,

    /// Flat attestor-set registration fee in `$LGT` micros. Set at genesis.
    #[state]
    pub attestor_set_fee: StateValue<u64>,

    /// Running counter of `$LGT` micros booked to the treasury.
    ///
    /// Accumulates the treasury share of every successful
    /// `SubmitAttestation` plus the full registration fees from
    /// `RegisterAttestorSet` and `RegisterSchema`. Today this is pure
    /// accounting; actual `sov-bank` token transfers land in a follow-up
    /// once the bank module is wired into the runtime.
    #[state]
    pub total_treasury_collected: StateValue<u64>,

    /// Running counter of `$LGT` micros booked per builder address.
    ///
    /// Keyed by `Schema::fee_routing_addr`. Incremented on each
    /// `SubmitAttestation` under a schema with non-zero `fee_routing_bps`
    /// by `total_fee * fee_routing_bps / 10000`. Same accounting-only
    /// caveat as [`AttestationModule::total_treasury_collected`].
    #[state]
    pub builder_fees_collected: StateMap<Address, u64>,
}

// ----- Protocol constants -----------------------------------------------------

/// Maximum `fee_routing_bps` accepted at schema registration (5000 = 50%).
/// Governance-adjustable in v1+; hardcoded for v0.
pub const MAX_BUILDER_BPS: u16 = 5000;

/// Default attestation fee in $LGT, in the chain's smallest unit.
/// Placeholder constant. Real fee sizing is a governance parameter set
/// at launch. Documented here for reference against the spec.
pub const DEFAULT_ATTESTATION_FEE_LGT_MICROS: u64 = 1_000; // 0.001 $LGT at 6 decimals

/// Default schema-registration fee in `$LGT` micros. 100 $LGT at 6 decimals.
pub const DEFAULT_SCHEMA_REGISTRATION_FEE_LGT_MICROS: u64 = 100_000_000;

/// Default attestor-set registration fee in `$LGT` micros. 10 $LGT at 6 decimals.
pub const DEFAULT_ATTESTOR_SET_FEE_LGT_MICROS: u64 = 10_000_000;

// ============================================================================
// Genesis configuration
// ============================================================================

/// An attestor set declared in the genesis config.
///
/// Validated with the same rules as [`CallMessage::RegisterAttestorSet`]:
/// non-empty members, `1 <= threshold <= members.len()`, derived id must
/// not collide with another attestor set already registered at genesis
/// time.
#[derive(Clone, Debug, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct InitialAttestorSet {
    /// Attestor public keys.
    pub members: Vec<PubKey>,
    /// M-of-N threshold.
    pub threshold: u8,
}

/// A schema declared in the genesis config.
///
/// Validated with the same rules as [`CallMessage::RegisterSchema`]: the
/// referenced attestor set must be known (either declared in
/// [`AttestationConfig::initial_attestor_sets`] or registered earlier in
/// the genesis flow), `fee_routing_bps <= MAX_BUILDER_BPS`, and the
/// address / bps orphan check. `owner` is supplied explicitly at genesis
/// because there is no transaction sender at that point.
#[derive(Clone, Debug, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct InitialSchema {
    /// Owner address assigned on-chain.
    pub owner: Address,
    /// Schema name, e.g. `"themisra.proof-of-prompt"`.
    pub name: String,
    /// Monotonic version; bumping it creates a new schema.
    pub version: u32,
    /// [`AttestorSetId`] the schema binds to.
    pub attestor_set: AttestorSetId,
    /// Fee routing basis points (`0..=MAX_BUILDER_BPS`).
    pub fee_routing_bps: u16,
    /// Fee routing address; required iff `fee_routing_bps > 0`.
    pub fee_routing_addr: Option<Address>,
}

/// Genesis configuration loaded once at chain launch.
///
/// Operators supply one of these, typically as a JSON blob, via the
/// genesis ceremony. See `docs/development/devnet.md` for the operator
/// flow and `crates/modules/attestation/examples/genesis.example.json`
/// for a worked example.
#[derive(Clone, Debug, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct AttestationConfig {
    /// Address that will receive treasury fee flows. Must be set
    /// explicitly; the `Default` impl uses the zero address as a
    /// placeholder that is almost certainly not what you want.
    pub treasury: Address,
    /// Flat per-attestation fee in `$LGT` micros (6 decimals).
    pub attestation_fee: u64,
    /// Flat schema-registration fee in `$LGT` micros.
    pub schema_registration_fee: u64,
    /// Flat attestor-set registration fee in `$LGT` micros.
    pub attestor_set_fee: u64,
    /// Attestor sets to register at genesis, in order.
    pub initial_attestor_sets: Vec<InitialAttestorSet>,
    /// Schemas to register at genesis, in order. Each schema's
    /// `attestor_set` must be one registered earlier in the flow (either
    /// in `initial_attestor_sets` above, or by a previous `InitialSchema`
    /// bootstrap pass, though the latter is not supported today).
    pub initial_schemas: Vec<InitialSchema>,
}

impl Default for AttestationConfig {
    fn default() -> Self {
        Self {
            treasury: [0u8; 32],
            attestation_fee: DEFAULT_ATTESTATION_FEE_LGT_MICROS,
            schema_registration_fee: DEFAULT_SCHEMA_REGISTRATION_FEE_LGT_MICROS,
            attestor_set_fee: DEFAULT_ATTESTOR_SET_FEE_LGT_MICROS,
            initial_attestor_sets: vec![],
            initial_schemas: vec![],
        }
    }
}

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

    /// The same pubkey appeared twice in one submission's signatures vector.
    #[error("duplicate attestor pubkey in signatures")]
    DuplicateSignaturePubkey,

    /// A signature's pubkey was not a member of the schema's attestor set.
    #[error("signature pubkey is not in the schema's attestor set")]
    UnknownAttestorPubkey,

    /// A signature's pubkey bytes did not decode as a valid ed25519 point.
    #[error("signature pubkey is not a valid ed25519 point")]
    InvalidAttestorPubkey,

    /// A signature's raw bytes were not the 64 bytes ed25519 expects.
    #[error("signature length {actual} != expected 64 for ed25519")]
    BadSignatureLength {
        /// Length that was actually submitted.
        actual: usize,
    },

    /// A signature did not verify against the canonical attestation digest.
    #[error("signature did not verify against attestation digest")]
    InvalidSignature,

    /// Fewer valid signatures than the schema's attestor set requires.
    #[error("quorum not met: {provided} valid signatures, need {required}")]
    BelowThreshold {
        /// Count of valid signatures in the submission.
        provided: u8,
        /// Threshold required by the schema's attestor set.
        required: u8,
    },

    /// A schema referenced an attestor set that no longer exists at submit
    /// time. Should be unreachable given the register-time invariants, but
    /// surfaced as an explicit error to keep state-transition code honest.
    #[error("schema's attestor set is missing from state")]
    MissingAttestorSet,
}

// ============================================================================
// Module implementation
// ============================================================================

impl<C: Context> Module for AttestationModule<C> {
    type Context = C;
    type Config = AttestationConfig;
    type CallMessage = CallMessage;
    type Event = ();

    fn genesis(&self, config: &Self::Config, working_set: &mut WorkingSet<C>) -> Result<(), Error> {
        Ok(self.seed_from_config(config, working_set)?)
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
    /// Apply an [`AttestationConfig`] during [`Module::genesis`].
    ///
    /// Stores treasury and fee parameters in state, then registers each
    /// configured attestor set and schema with the same validation rules
    /// the `RegisterAttestorSet` and `RegisterSchema` handlers use at
    /// runtime. Rejects duplicates and orphan / over-cap fee routing.
    fn seed_from_config(
        &self,
        config: &AttestationConfig,
        working_set: &mut WorkingSet<C>,
    ) -> anyhow::Result<()> {
        self.treasury.set(&config.treasury, working_set);
        self.attestation_fee.set(&config.attestation_fee, working_set);
        self.schema_registration_fee.set(&config.schema_registration_fee, working_set);
        self.attestor_set_fee.set(&config.attestor_set_fee, working_set);

        for initial in &config.initial_attestor_sets {
            if initial.members.is_empty() {
                return Err(AttestationError::EmptyAttestorSet.into());
            }
            if initial.threshold == 0 {
                return Err(AttestationError::ZeroThreshold.into());
            }
            if usize::from(initial.threshold) > initial.members.len() {
                return Err(AttestationError::ThresholdExceedsMembers {
                    threshold: initial.threshold,
                    count: initial.members.len(),
                }
                .into());
            }

            let mut sorted_members = initial.members.clone();
            sorted_members.sort_unstable();

            let id = AttestorSet::derive_id(&sorted_members, initial.threshold);
            if self.attestor_sets.get(&id, working_set).is_some() {
                return Err(AttestationError::DuplicateAttestorSet.into());
            }
            self.attestor_sets.set(
                &id,
                &AttestorSet { members: sorted_members, threshold: initial.threshold },
                working_set,
            );
        }

        for schema in &config.initial_schemas {
            if schema.fee_routing_bps > MAX_BUILDER_BPS {
                return Err(AttestationError::FeeRoutingExceedsCap {
                    bps: schema.fee_routing_bps,
                    cap: MAX_BUILDER_BPS,
                }
                .into());
            }
            if (schema.fee_routing_bps > 0) != schema.fee_routing_addr.is_some() {
                return Err(AttestationError::OrphanFeeRouting.into());
            }
            if self.attestor_sets.get(&schema.attestor_set, working_set).is_none() {
                return Err(AttestationError::UnknownAttestorSet.into());
            }

            let id = Schema::derive_id(&schema.owner, &schema.name, schema.version);
            if self.schemas.get(&id, working_set).is_some() {
                return Err(AttestationError::DuplicateSchema.into());
            }
            self.schemas.set(
                &id,
                &Schema {
                    owner: schema.owner,
                    name: schema.name.clone(),
                    version: schema.version,
                    attestor_set: schema.attestor_set,
                    fee_routing_bps: schema.fee_routing_bps,
                    fee_routing_addr: schema.fee_routing_addr,
                },
                working_set,
            );
        }

        Ok(())
    }

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

        let fee =
            self.attestor_set_fee.get(working_set).unwrap_or(DEFAULT_ATTESTOR_SET_FEE_LGT_MICROS);
        self.add_to_treasury(fee, working_set);

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

        let fee = self
            .schema_registration_fee
            .get(working_set)
            .unwrap_or(DEFAULT_SCHEMA_REGISTRATION_FEE_LGT_MICROS);
        self.add_to_treasury(fee, working_set);

        Ok(CallResponse::default())
    }

    /// Handle [`CallMessage::SubmitAttestation`].
    ///
    /// Enforces:
    /// 1. Schema exists.
    /// 2. `(schema_id, payload_hash)` has not been written before.
    /// 3. Every signature is valid ed25519 over the canonical
    ///    [`SignedAttestationPayload`] digest.
    /// 4. Every signing pubkey is a member of the schema's [`AttestorSet`].
    /// 5. No pubkey appears twice in the submission.
    /// 6. Count of valid signatures is at least the attestor set's
    ///    `threshold`.
    ///
    /// Fee charging is still stubbed here; #22 owns it.
    fn handle_submit_attestation(
        &self,
        schema_id: SchemaId,
        payload_hash: Hash32,
        signatures: Vec<AttestorSignature>,
        context: &C,
        working_set: &mut WorkingSet<C>,
    ) -> anyhow::Result<CallResponse> {
        let schema =
            self.schemas.get(&schema_id, working_set).ok_or(AttestationError::UnknownSchema)?;

        let key = (schema_id, payload_hash);
        if self.attestations.get(&key, working_set).is_some() {
            return Err(AttestationError::DuplicateAttestation.into());
        }

        let attestor_set = self
            .attestor_sets
            .get(&schema.attestor_set, working_set)
            .ok_or(AttestationError::MissingAttestorSet)?;

        let submitter = sender_to_address::<C>(context.sender())?;
        // TODO: pull the real block timestamp once WorkingSet exposes it
        // (slot/header access lives behind the rollup-blueprint layer and is
        // wired in #13). For now, attestations carry `timestamp = 0`; the
        // write-once key prevents replay either way.
        let timestamp = 0u64;

        // Canonical digest attestors sign over. Must be computed after we
        // know `submitter` and `timestamp`; those are part of what's signed.
        let digest =
            SignedAttestationPayload { schema_id, payload_hash, submitter, timestamp }.digest();

        validate_signatures(&signatures, &attestor_set, &digest)?;

        self.attestations.set(
            &key,
            &Attestation { schema_id, payload_hash, submitter, timestamp, signatures },
            working_set,
        );

        // Charge the attestation fee. Split per schema.fee_routing_bps:
        // builder share to schema.fee_routing_addr (if any), remainder to
        // treasury. Today this is accounting-only against module state;
        // real `sov-bank` token movement lands once the bank module is
        // wired into the runtime.
        let total_fee =
            self.attestation_fee.get(working_set).unwrap_or(DEFAULT_ATTESTATION_FEE_LGT_MICROS);
        let (builder_share, treasury_share) =
            split_attestation_fee(total_fee, schema.fee_routing_bps);
        if let Some(addr) = schema.fee_routing_addr {
            if builder_share > 0 {
                self.add_to_builder(addr, builder_share, working_set);
            }
        }
        self.add_to_treasury(treasury_share, working_set);

        Ok(CallResponse::default())
    }

    /// Increment [`AttestationModule::total_treasury_collected`] by `amount`,
    /// saturating at `u64::MAX`.
    fn add_to_treasury(&self, amount: u64, working_set: &mut WorkingSet<C>) {
        let current = self.total_treasury_collected.get(working_set).unwrap_or(0);
        self.total_treasury_collected.set(&current.saturating_add(amount), working_set);
    }

    /// Increment [`AttestationModule::builder_fees_collected`] for `addr` by
    /// `amount`, saturating at `u64::MAX`.
    fn add_to_builder(&self, addr: Address, amount: u64, working_set: &mut WorkingSet<C>) {
        let current = self.builder_fees_collected.get(&addr, working_set).unwrap_or(0);
        self.builder_fees_collected.set(&addr, &current.saturating_add(amount), working_set);
    }
}

/// Split an attestation fee into `(builder_share, treasury_share)` per
/// `fee_routing_bps` (basis points, capped at [`MAX_BUILDER_BPS`]).
///
/// Uses a `u128` intermediate to avoid overflow on `bps * total`. With
/// `bps <= 5000`, `total <= u64::MAX`, the intermediate fits comfortably
/// in `u128` and the share casts back to `u64` losslessly.
fn split_attestation_fee(total: u64, bps: u16) -> (u64, u64) {
    let bps_u128 = u128::from(bps);
    let total_u128 = u128::from(total);
    let builder = (bps_u128 * total_u128 / 10_000) as u64;
    let treasury = total.saturating_sub(builder);
    (builder, treasury)
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

/// Validate a submission's [`AttestorSignature`] vector against an
/// [`AttestorSet`] and the canonical attestation digest.
///
/// Enforces, in order:
/// 1. No duplicate pubkey within the submission.
/// 2. Each pubkey is a member of `set.members`.
/// 3. Each pubkey bytes decode as a valid ed25519 point.
/// 4. Each signature's raw bytes are 64 bytes long.
/// 5. The ed25519 signature verifies against `digest` for its pubkey.
/// 6. The count of valid signatures is at least `set.threshold`.
///
/// Ed25519 is the only scheme accepted in v0. BLS aggregates or other
/// schemes would be added as separate variants later; `AttestorSignature.sig`
/// is `Vec<u8>` today precisely to leave room for that.
fn validate_signatures(
    signatures: &[AttestorSignature],
    set: &AttestorSet,
    digest: &Hash32,
) -> anyhow::Result<()> {
    use std::collections::BTreeSet;

    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    let members: BTreeSet<&PubKey> = set.members.iter().collect();
    let mut seen: BTreeSet<PubKey> = BTreeSet::new();
    let mut valid: u16 = 0;

    for attested in signatures {
        if !seen.insert(attested.pubkey) {
            return Err(AttestationError::DuplicateSignaturePubkey.into());
        }

        if !members.contains(&attested.pubkey) {
            return Err(AttestationError::UnknownAttestorPubkey.into());
        }

        let verifying_key = VerifyingKey::from_bytes(&attested.pubkey)
            .map_err(|_| AttestationError::InvalidAttestorPubkey)?;

        let sig_bytes: [u8; 64] = attested
            .sig
            .as_slice()
            .try_into()
            .map_err(|_| AttestationError::BadSignatureLength { actual: attested.sig.len() })?;
        let signature = Signature::from_bytes(&sig_bytes);

        if verifying_key.verify(digest, &signature).is_err() {
            return Err(AttestationError::InvalidSignature.into());
        }

        valid = valid.saturating_add(1);
    }

    let valid_u8: u8 = valid.min(u16::from(u8::MAX)) as u8;
    if valid_u8 < set.threshold {
        return Err(AttestationError::BelowThreshold {
            provided: valid_u8,
            required: set.threshold,
        }
        .into());
    }

    Ok(())
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

    // ------ Fee split math -----------------------------------------------

    #[test]
    fn split_zero_bps_routes_full_amount_to_treasury() {
        let (builder, treasury) = split_attestation_fee(1_000, 0);
        assert_eq!(builder, 0);
        assert_eq!(treasury, 1_000);
    }

    #[test]
    fn split_25pct_bps() {
        let (builder, treasury) = split_attestation_fee(1_000, 2500);
        assert_eq!(builder, 250);
        assert_eq!(treasury, 750);
    }

    #[test]
    fn split_at_protocol_cap_is_50_50() {
        let (builder, treasury) = split_attestation_fee(1_000, MAX_BUILDER_BPS);
        assert_eq!(builder, 500);
        assert_eq!(treasury, 500);
    }

    #[test]
    fn split_handles_max_total_without_overflow() {
        // Without the u128 intermediate, 5000 * u64::MAX would wrap.
        // We just need to confirm it does not panic and the parts sum
        // back to total without underflow.
        let total = u64::MAX;
        let (builder, treasury) = split_attestation_fee(total, MAX_BUILDER_BPS);
        assert!(builder <= total);
        assert_eq!(builder.saturating_add(treasury), total);
    }
}

// ============================================================================
// State-transition tests (end-to-end via module.call)
// ============================================================================

#[cfg(test)]
mod state_transition_tests {
    use ed25519_dalek::{Signer, SigningKey};
    use sov_modules_api::default_context::DefaultContext;
    use sov_modules_api::{Address as SovAddress, Module as _, WorkingSet};
    use sov_prover_storage_manager::new_orphan_storage;

    use super::*;

    type TestModule = AttestationModule<DefaultContext>;

    /// Address the `fresh()` context uses as `sender`; also the owner of
    /// any schema registered inside a test using that context.
    const TEST_SENDER: Address = [1u8; 32];

    fn fresh() -> (TestModule, DefaultContext, WorkingSet<DefaultContext>, tempfile::TempDir) {
        let tmpdir = tempfile::tempdir().unwrap();
        let working_set = WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
        let sender = SovAddress::from(TEST_SENDER);
        let sequencer = SovAddress::from([9u8; 32]);
        let context = DefaultContext::new(sender, sequencer, 1);
        let module = TestModule::default();
        (module, context, working_set, tmpdir)
    }

    /// Deterministic signers seeded from `[i; 32]`. Keeps test sigs
    /// reproducible. Identity of each signer is stable across runs.
    fn sample_signers() -> Vec<SigningKey> {
        (1u8..=3).map(|i| SigningKey::from_bytes(&[i; 32])).collect()
    }

    fn members_of(signers: &[SigningKey]) -> Vec<PubKey> {
        signers.iter().map(|sk| sk.verifying_key().to_bytes()).collect()
    }

    fn sample_members() -> Vec<PubKey> {
        members_of(&sample_signers())
    }

    /// Canonical attestation digest matching what the module computes at
    /// submit time. `submitter` in tests is always [`TEST_SENDER`];
    /// `timestamp` is still 0 (block-header access lands with #13).
    fn digest_for(schema_id: SchemaId, payload_hash: Hash32) -> Hash32 {
        SignedAttestationPayload { schema_id, payload_hash, submitter: TEST_SENDER, timestamp: 0 }
            .digest()
    }

    fn sign_with(sk: &SigningKey, digest: &Hash32) -> AttestorSignature {
        AttestorSignature {
            pubkey: sk.verifying_key().to_bytes(),
            sig: sk.sign(digest).to_bytes().to_vec(),
        }
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
        let signers = sample_signers();

        // 1. Register attestor set (threshold 2 of 3)
        module
            .call(
                CallMessage::RegisterAttestorSet { members: members_of(&signers), threshold: 2 },
                &context,
                &mut ws,
            )
            .expect("register attestor set");
        let attestor_set_id = AttestorSet::derive_id(&members_of(&signers), 2);

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
        let schema_id = Schema::derive_id(&TEST_SENDER, "themisra.proof-of-prompt", 1);

        // 3. Submit attestation signed by 2 of 3 (meets threshold)
        let payload_hash = [0x42u8; 32];
        let digest = digest_for(schema_id, payload_hash);
        let signatures = vec![sign_with(&signers[0], &digest), sign_with(&signers[1], &digest)];

        module
            .call(
                CallMessage::SubmitAttestation { schema_id, payload_hash, signatures },
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
        assert_eq!(stored.submitter, TEST_SENDER);

        // 5. Write-once: resubmitting same (schema_id, payload_hash) must fail
        let res = module.call(
            CallMessage::SubmitAttestation { schema_id, payload_hash, signatures: vec![] },
            &context,
            &mut ws,
        );
        assert!(res.is_err(), "duplicate (schema_id, payload_hash) must be rejected");
    }

    // --------- Signature validation reject paths -----------------------------

    /// Set up schema + attestor set and return (schema_id, digest_for_hash).
    /// Threshold 2 of 3. Used by every signature-validation test below.
    fn sig_test_setup(
        module: &TestModule,
        context: &DefaultContext,
        ws: &mut WorkingSet<DefaultContext>,
        payload_hash: Hash32,
    ) -> (SchemaId, Hash32) {
        let signers = sample_signers();
        module
            .call(
                CallMessage::RegisterAttestorSet { members: members_of(&signers), threshold: 2 },
                context,
                ws,
            )
            .unwrap();
        let attestor_set_id = AttestorSet::derive_id(&members_of(&signers), 2);
        module
            .call(
                CallMessage::RegisterSchema {
                    name: "sig-test".into(),
                    version: 1,
                    attestor_set: attestor_set_id,
                    fee_routing_bps: 0,
                    fee_routing_addr: None,
                },
                context,
                ws,
            )
            .unwrap();
        let schema_id = Schema::derive_id(&TEST_SENDER, "sig-test", 1);
        let digest = digest_for(schema_id, payload_hash);
        (schema_id, digest)
    }

    #[test]
    fn submit_below_threshold_rejects() {
        let (module, context, mut ws, _td) = fresh();
        let payload_hash = [0x01u8; 32];
        let (schema_id, digest) = sig_test_setup(&module, &context, &mut ws, payload_hash);
        let signers = sample_signers();

        // Only 1 signature, threshold is 2.
        let signatures = vec![sign_with(&signers[0], &digest)];
        let res = module.call(
            CallMessage::SubmitAttestation { schema_id, payload_hash, signatures },
            &context,
            &mut ws,
        );
        assert!(res.is_err(), "expected BelowThreshold");
    }

    #[test]
    fn submit_with_unknown_pubkey_rejects() {
        let (module, context, mut ws, _td) = fresh();
        let payload_hash = [0x02u8; 32];
        let (schema_id, digest) = sig_test_setup(&module, &context, &mut ws, payload_hash);
        let signers = sample_signers();

        // One legit sig, one signed by a stranger not in the attestor set.
        let stranger = SigningKey::from_bytes(&[99u8; 32]);
        let signatures = vec![sign_with(&signers[0], &digest), sign_with(&stranger, &digest)];
        let res = module.call(
            CallMessage::SubmitAttestation { schema_id, payload_hash, signatures },
            &context,
            &mut ws,
        );
        assert!(res.is_err(), "expected UnknownAttestorPubkey");
    }

    #[test]
    fn submit_with_invalid_signature_rejects() {
        let (module, context, mut ws, _td) = fresh();
        let payload_hash = [0x03u8; 32];
        let (schema_id, digest) = sig_test_setup(&module, &context, &mut ws, payload_hash);
        let signers = sample_signers();

        // Valid sig + one whose bytes are corrupted (flip a bit).
        let mut corrupted = sign_with(&signers[0], &digest);
        corrupted.sig[0] ^= 0x01;
        let signatures = vec![sign_with(&signers[1], &digest), corrupted];
        let res = module.call(
            CallMessage::SubmitAttestation { schema_id, payload_hash, signatures },
            &context,
            &mut ws,
        );
        assert!(res.is_err(), "expected InvalidSignature");
    }

    #[test]
    fn submit_with_duplicate_pubkey_rejects() {
        let (module, context, mut ws, _td) = fresh();
        let payload_hash = [0x04u8; 32];
        let (schema_id, digest) = sig_test_setup(&module, &context, &mut ws, payload_hash);
        let signers = sample_signers();

        // Same signer twice, even though both signatures are technically valid.
        let s = sign_with(&signers[0], &digest);
        let signatures = vec![s.clone(), s];
        let res = module.call(
            CallMessage::SubmitAttestation { schema_id, payload_hash, signatures },
            &context,
            &mut ws,
        );
        assert!(res.is_err(), "expected DuplicateSignaturePubkey");
    }

    #[test]
    fn submit_with_bad_sig_length_rejects() {
        let (module, context, mut ws, _td) = fresh();
        let payload_hash = [0x05u8; 32];
        let (schema_id, digest) = sig_test_setup(&module, &context, &mut ws, payload_hash);
        let signers = sample_signers();

        // Take a legit sig but truncate the raw bytes.
        let mut truncated = sign_with(&signers[0], &digest);
        truncated.sig.truncate(63);
        let signatures = vec![sign_with(&signers[1], &digest), truncated];
        let res = module.call(
            CallMessage::SubmitAttestation { schema_id, payload_hash, signatures },
            &context,
            &mut ws,
        );
        assert!(res.is_err(), "expected BadSignatureLength");
    }

    #[test]
    fn submit_digest_is_bound_to_submitter_and_timestamp() {
        // Signing the wrong submitter produces a signature that doesn't
        // verify against the module-computed digest. This locks in the
        // invariant that submitter and timestamp are part of what attestors
        // sign, not caller-controlled free variables.
        let (module, context, mut ws, _td) = fresh();
        let payload_hash = [0x06u8; 32];
        let (schema_id, _correct_digest) = sig_test_setup(&module, &context, &mut ws, payload_hash);
        let signers = sample_signers();

        let wrong_submitter_digest = SignedAttestationPayload {
            schema_id,
            payload_hash,
            submitter: [0xFFu8; 32], // not TEST_SENDER
            timestamp: 0,
        }
        .digest();

        let signatures = vec![
            sign_with(&signers[0], &wrong_submitter_digest),
            sign_with(&signers[1], &wrong_submitter_digest),
        ];
        let res = module.call(
            CallMessage::SubmitAttestation { schema_id, payload_hash, signatures },
            &context,
            &mut ws,
        );
        assert!(res.is_err(), "digest bound to submitter should reject mis-signed attestation");
    }
}

// ============================================================================
// Genesis tests
// ============================================================================

#[cfg(test)]
mod genesis_tests {
    use sov_modules_api::default_context::DefaultContext;
    use sov_modules_api::{Module as _, WorkingSet};
    use sov_prover_storage_manager::new_orphan_storage;

    use super::*;

    type TestModule = AttestationModule<DefaultContext>;

    fn fresh() -> (TestModule, WorkingSet<DefaultContext>, tempfile::TempDir) {
        let tmpdir = tempfile::tempdir().unwrap();
        let working_set = WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
        let module = TestModule::default();
        (module, working_set, tmpdir)
    }

    #[test]
    fn genesis_with_default_config_seeds_treasury_and_fees() {
        let (module, mut ws, _td) = fresh();
        let cfg = AttestationConfig::default();
        module.genesis(&cfg, &mut ws).expect("genesis should succeed");

        assert_eq!(module.treasury.get(&mut ws), Some(cfg.treasury));
        assert_eq!(module.attestation_fee.get(&mut ws), Some(cfg.attestation_fee));
        assert_eq!(module.schema_registration_fee.get(&mut ws), Some(cfg.schema_registration_fee));
        assert_eq!(module.attestor_set_fee.get(&mut ws), Some(cfg.attestor_set_fee));
    }

    #[test]
    fn genesis_seeds_initial_attestor_sets_and_schemas() {
        let (module, mut ws, _td) = fresh();
        let members = vec![[1u8; 32], [2u8; 32], [3u8; 32]];
        let attestor_set_id = AttestorSet::derive_id(&members, 2);
        let owner = [42u8; 32];
        let schema_name = "themisra.proof-of-prompt".to_string();
        let schema_id = Schema::derive_id(&owner, &schema_name, 1);

        let cfg = AttestationConfig {
            initial_attestor_sets: vec![InitialAttestorSet {
                members: members.clone(),
                threshold: 2,
            }],
            initial_schemas: vec![InitialSchema {
                owner,
                name: schema_name,
                version: 1,
                attestor_set: attestor_set_id,
                fee_routing_bps: 0,
                fee_routing_addr: None,
            }],
            ..AttestationConfig::default()
        };

        module.genesis(&cfg, &mut ws).expect("genesis should succeed");

        assert!(module.attestor_sets.get(&attestor_set_id, &mut ws).is_some());
        assert!(module.schemas.get(&schema_id, &mut ws).is_some());
    }

    #[test]
    fn genesis_rejects_zero_threshold_in_initial_set() {
        let (module, mut ws, _td) = fresh();
        let cfg = AttestationConfig {
            initial_attestor_sets: vec![InitialAttestorSet {
                members: vec![[1u8; 32], [2u8; 32]],
                threshold: 0,
            }],
            ..AttestationConfig::default()
        };
        assert!(module.genesis(&cfg, &mut ws).is_err());
    }

    #[test]
    fn genesis_rejects_threshold_over_members_in_initial_set() {
        let (module, mut ws, _td) = fresh();
        let cfg = AttestationConfig {
            initial_attestor_sets: vec![InitialAttestorSet {
                members: vec![[1u8; 32]],
                threshold: 2,
            }],
            ..AttestationConfig::default()
        };
        assert!(module.genesis(&cfg, &mut ws).is_err());
    }

    #[test]
    fn genesis_rejects_schema_with_unknown_attestor_set() {
        let (module, mut ws, _td) = fresh();
        let cfg = AttestationConfig {
            initial_schemas: vec![InitialSchema {
                owner: [1u8; 32],
                name: "foo".into(),
                version: 1,
                attestor_set: [0xAAu8; 32], // not registered
                fee_routing_bps: 0,
                fee_routing_addr: None,
            }],
            ..AttestationConfig::default()
        };
        assert!(module.genesis(&cfg, &mut ws).is_err());
    }

    #[test]
    fn genesis_rejects_schema_with_over_cap_fee_routing() {
        let (module, mut ws, _td) = fresh();
        let members = vec![[1u8; 32], [2u8; 32]];
        let attestor_set_id = AttestorSet::derive_id(&members, 2);
        let cfg = AttestationConfig {
            initial_attestor_sets: vec![InitialAttestorSet { members, threshold: 2 }],
            initial_schemas: vec![InitialSchema {
                owner: [1u8; 32],
                name: "foo".into(),
                version: 1,
                attestor_set: attestor_set_id,
                fee_routing_bps: 6000, // over MAX_BUILDER_BPS
                fee_routing_addr: Some([2u8; 32]),
            }],
            ..AttestationConfig::default()
        };
        assert!(module.genesis(&cfg, &mut ws).is_err());
    }

    #[test]
    fn genesis_rejects_orphan_fee_routing() {
        let (module, mut ws, _td) = fresh();
        let members = vec![[1u8; 32], [2u8; 32]];
        let attestor_set_id = AttestorSet::derive_id(&members, 2);
        let cfg = AttestationConfig {
            initial_attestor_sets: vec![InitialAttestorSet { members, threshold: 2 }],
            initial_schemas: vec![InitialSchema {
                owner: [1u8; 32],
                name: "foo".into(),
                version: 1,
                attestor_set: attestor_set_id,
                fee_routing_bps: 1000,
                fee_routing_addr: None, // orphan: bps > 0 with no addr
            }],
            ..AttestationConfig::default()
        };
        assert!(module.genesis(&cfg, &mut ws).is_err());
    }

    #[test]
    fn config_round_trips_through_json() {
        let cfg = AttestationConfig {
            treasury: [9u8; 32],
            attestation_fee: 1234,
            schema_registration_fee: 5678,
            attestor_set_fee: 91011,
            initial_attestor_sets: vec![InitialAttestorSet {
                members: vec![[1u8; 32], [2u8; 32]],
                threshold: 2,
            }],
            initial_schemas: vec![InitialSchema {
                owner: [3u8; 32],
                name: "demo".into(),
                version: 1,
                attestor_set: [4u8; 32],
                fee_routing_bps: 1000,
                fee_routing_addr: Some([5u8; 32]),
            }],
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let decoded: AttestationConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.treasury, cfg.treasury);
        assert_eq!(decoded.initial_attestor_sets.len(), 1);
        assert_eq!(decoded.initial_schemas.len(), 1);
    }
}

// ============================================================================
// Fee accounting tests (handler integration)
// ============================================================================

#[cfg(test)]
mod fee_tests {
    use ed25519_dalek::{Signer, SigningKey};
    use sov_modules_api::default_context::DefaultContext;
    use sov_modules_api::{Address as SovAddress, Module as _, WorkingSet};
    use sov_prover_storage_manager::new_orphan_storage;

    use super::*;

    type TestModule = AttestationModule<DefaultContext>;

    const TEST_SENDER: Address = [1u8; 32];
    const TREASURY: Address = [11u8; 32];
    const BUILDER_PAYOUT: Address = [22u8; 32];

    fn fresh() -> (TestModule, DefaultContext, WorkingSet<DefaultContext>, tempfile::TempDir) {
        let tmpdir = tempfile::tempdir().unwrap();
        let working_set = WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
        let sender = SovAddress::from(TEST_SENDER);
        let sequencer = SovAddress::from([9u8; 32]);
        let context = DefaultContext::new(sender, sequencer, 1);
        let module = TestModule::default();
        (module, context, working_set, tmpdir)
    }

    fn signers() -> Vec<SigningKey> {
        (1u8..=3).map(|i| SigningKey::from_bytes(&[i; 32])).collect()
    }

    fn members(sks: &[SigningKey]) -> Vec<PubKey> {
        sks.iter().map(|sk| sk.verifying_key().to_bytes()).collect()
    }

    fn sign_with(sk: &SigningKey, digest: &Hash32) -> AttestorSignature {
        AttestorSignature {
            pubkey: sk.verifying_key().to_bytes(),
            sig: sk.sign(digest).to_bytes().to_vec(),
        }
    }

    fn config_with_fees(
        attestation_fee: u64,
        schema_fee: u64,
        attestor_set_fee: u64,
    ) -> AttestationConfig {
        AttestationConfig {
            treasury: TREASURY,
            attestation_fee,
            schema_registration_fee: schema_fee,
            attestor_set_fee,
            initial_attestor_sets: vec![],
            initial_schemas: vec![],
        }
    }

    #[test]
    fn register_attestor_set_charges_treasury() {
        let (module, context, mut ws, _td) = fresh();
        module.genesis(&config_with_fees(0, 0, 7), &mut ws).unwrap();

        module
            .call(
                CallMessage::RegisterAttestorSet { members: members(&signers()), threshold: 2 },
                &context,
                &mut ws,
            )
            .unwrap();

        assert_eq!(module.total_treasury_collected.get(&mut ws), Some(7));
    }

    #[test]
    fn register_schema_charges_treasury() {
        let (module, context, mut ws, _td) = fresh();
        // Set the schema fee to 100; attestor-set fee is 5 so we can also
        // confirm both increments accumulate.
        module.genesis(&config_with_fees(0, 100, 5), &mut ws).unwrap();

        let signers = signers();
        let set_members = members(&signers);
        module
            .call(
                CallMessage::RegisterAttestorSet { members: set_members.clone(), threshold: 2 },
                &context,
                &mut ws,
            )
            .unwrap();
        let attestor_set_id = AttestorSet::derive_id(&set_members, 2);

        module
            .call(
                CallMessage::RegisterSchema {
                    name: "test.schema".into(),
                    version: 1,
                    attestor_set: attestor_set_id,
                    fee_routing_bps: 0,
                    fee_routing_addr: None,
                },
                &context,
                &mut ws,
            )
            .unwrap();

        // 5 (attestor set) + 100 (schema) = 105.
        assert_eq!(module.total_treasury_collected.get(&mut ws), Some(105));
    }

    #[test]
    fn submit_attestation_full_amount_to_treasury_when_no_routing() {
        let (module, context, mut ws, _td) = fresh();
        module.genesis(&config_with_fees(1_000, 0, 0), &mut ws).unwrap();
        let signers = signers();
        let set_members = members(&signers);
        module
            .call(
                CallMessage::RegisterAttestorSet { members: set_members.clone(), threshold: 2 },
                &context,
                &mut ws,
            )
            .unwrap();
        let attestor_set_id = AttestorSet::derive_id(&set_members, 2);
        module
            .call(
                CallMessage::RegisterSchema {
                    name: "no-routing".into(),
                    version: 1,
                    attestor_set: attestor_set_id,
                    fee_routing_bps: 0,
                    fee_routing_addr: None,
                },
                &context,
                &mut ws,
            )
            .unwrap();
        let schema_id = Schema::derive_id(&TEST_SENDER, "no-routing", 1);

        // Treasury counter at this point: 0 + 0 + 0 = 0 (genesis fees were
        // all set to 0 except attestation_fee which only kicks in on submit).
        let baseline = module.total_treasury_collected.get(&mut ws).unwrap_or(0);

        let payload_hash = [0xAAu8; 32];
        let digest = SignedAttestationPayload {
            schema_id,
            payload_hash,
            submitter: TEST_SENDER,
            timestamp: 0,
        }
        .digest();
        module
            .call(
                CallMessage::SubmitAttestation {
                    schema_id,
                    payload_hash,
                    signatures: vec![
                        sign_with(&signers[0], &digest),
                        sign_with(&signers[1], &digest),
                    ],
                },
                &context,
                &mut ws,
            )
            .unwrap();

        assert_eq!(module.total_treasury_collected.get(&mut ws), Some(baseline + 1_000));
        // Builder counter for the un-set routing addr should be untouched.
        assert!(module.builder_fees_collected.get(&BUILDER_PAYOUT, &mut ws).is_none());
    }

    #[test]
    fn submit_attestation_splits_with_routing() {
        let (module, context, mut ws, _td) = fresh();
        module.genesis(&config_with_fees(1_000, 0, 0), &mut ws).unwrap();
        let signers = signers();
        let set_members = members(&signers);
        module
            .call(
                CallMessage::RegisterAttestorSet { members: set_members.clone(), threshold: 2 },
                &context,
                &mut ws,
            )
            .unwrap();
        let attestor_set_id = AttestorSet::derive_id(&set_members, 2);
        module
            .call(
                CallMessage::RegisterSchema {
                    name: "routed".into(),
                    version: 1,
                    attestor_set: attestor_set_id,
                    fee_routing_bps: 2500, // 25% to builder
                    fee_routing_addr: Some(BUILDER_PAYOUT),
                },
                &context,
                &mut ws,
            )
            .unwrap();
        let schema_id = Schema::derive_id(&TEST_SENDER, "routed", 1);

        let baseline_treasury = module.total_treasury_collected.get(&mut ws).unwrap_or(0);

        let payload_hash = [0xBBu8; 32];
        let digest = SignedAttestationPayload {
            schema_id,
            payload_hash,
            submitter: TEST_SENDER,
            timestamp: 0,
        }
        .digest();
        module
            .call(
                CallMessage::SubmitAttestation {
                    schema_id,
                    payload_hash,
                    signatures: vec![
                        sign_with(&signers[0], &digest),
                        sign_with(&signers[1], &digest),
                    ],
                },
                &context,
                &mut ws,
            )
            .unwrap();

        // 25% of 1000 = 250 to builder, remainder 750 to treasury.
        assert_eq!(module.builder_fees_collected.get(&BUILDER_PAYOUT, &mut ws), Some(250));
        assert_eq!(module.total_treasury_collected.get(&mut ws), Some(baseline_treasury + 750));
    }
}
