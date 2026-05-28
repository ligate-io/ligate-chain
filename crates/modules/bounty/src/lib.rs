//! Ligate bounty marketplace, on-chain bounty primitive composed on the
//! [`attestation`] module.
//!
//! See [`docs/protocol/bounty-marketplace.md`][spec] for the full RFC.
//!
//! [spec]: https://github.com/ligate-io/ligate-chain/blob/main/docs/protocol/bounty-marketplace.md
//!
//! # v0 skeleton scope
//!
//! This crate ships the module declarations, state shape, [`CallMessage`]
//! variants, and [`Event`] variants exactly as the RFC describes. Handler
//! bodies are no-ops at v0 ([`Ok`](Result::Ok)-returning stubs); real
//! state-transition logic lands in follow-up PRs tracked under
//! [chain#519](https://github.com/ligate-io/ligate-chain/issues/519).
//!
//! The reason for the no-op-first split: adding the crate to the runtime
//! composition is a `chain_hash`-changing change (new module in the borsh
//! schema fingerprint), and we want that change to ship cleanly through a
//! single release rather than thread it through every handler PR.
//!
//! # Module shape
//!
//! Four `StateMap`s mirroring the RFC's "State" section, plus five
//! [`CallMessage`] variants for the lifecycle. Bounty boards are
//! [`attestation::Schema`]s; this module's state references their
//! [`SchemaId`] but does not duplicate schema data.

#![deny(missing_docs)]

#[cfg(feature = "native")]
mod query;

#[cfg(feature = "native")]
pub use query::BountyResponse;

#[cfg(feature = "native")]
pub mod metrics;

use attestation::{AttestationId, AttestorSetId, PayloadHash, SchemaId};
use borsh::{BorshDeserialize, BorshSerialize};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sov_bank::Amount;
use sov_modules_api::macros::UniversalWallet;
use sov_modules_api::{
    Context, DaSpec, GenesisState, Module, ModuleId, ModuleInfo, ModuleRestApi, SafeVec, Spec,
    StateMap, TxState,
};
use thiserror::Error as ThisError;

// ----- Bound parameters on CallMessage payloads -------------------------------

/// Hard cap on `ClaimBounty.claims` length, per-tx claim batch.
pub const MAX_CLAIMS_PER_CALL: usize = 64;

/// Hard cap on `AcceptancePredicate::PayloadHashes` list length.
pub const MAX_PAYLOAD_HASHES: usize = 256;

/// Hard cap on `Bounty::open_by_schema` reverse-index list length.
pub const MAX_OPEN_BOUNTIES_PER_SCHEMA: usize = 1024;

// ============================================================================
// Identifier types
// ============================================================================

// `BountyId`: 32-byte hash with the `lbt` Bech32m prefix. Follows the
// same shape as the attestation module's `lsc/las/lph/lpk/lat` family.
// Prefix reservation tracked under chain#46.
mod bounty_id_mod {
    sov_modules_api::impl_hash32_type!(BountyId, BountyIdBech32, "lbt");
}
pub use bounty_id_mod::{BountyId, BountyIdBech32};

impl BountyId {
    /// Compute the deterministic [`BountyId`] from the call inputs.
    ///
    /// `SHA-256(poster_bytes ‖ board_schema_id_bytes ‖ nonce_le)`.
    pub fn derive<A: AsRef<[u8]>>(poster: A, board_schema_id: &SchemaId, nonce: u64) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(poster.as_ref());
        hasher.update(board_schema_id.as_ref());
        hasher.update(nonce.to_le_bytes());
        let digest = hasher.finalize();
        let mut out = [0u8; 32];
        out.copy_from_slice(&digest);
        Self::from(out)
    }
}

// ============================================================================
// Lifecycle status
// ============================================================================

/// Lifecycle status of a bounty.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
    JsonSchema,
    UniversalWallet,
)]
pub enum BountyStatus {
    /// Posted and funded; accepting claims.
    Open,
    /// Pool fully consumed by claims; waiting on dispute window.
    Exhausted,
    /// Expired without full payout; remaining escrow refundable.
    Expired,
    /// Poster cancelled the bounty.
    Cancelled,
    /// All claims paid out and dispute window closed.
    Finalised,
}

// ============================================================================
// Acceptance predicate
// ============================================================================

/// Rule the chain uses to decide which attestations count as valid claims.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
    JsonSchema,
    UniversalWallet,
)]
pub enum AcceptancePredicate {
    /// Any valid attestation against the board's schema counts.
    Any,
    /// Only attestations signed by attestors in this set count.
    AttestorSet(AttestorSetId),
    /// Only attestations whose `payload_hash` matches one of these.
    PayloadHashes(SafeVec<PayloadHash, MAX_PAYLOAD_HASHES>),
    /// At least `min_attestors` peer attestations of the same payload hash.
    PeerCount {
        /// Minimum distinct-attestor count required.
        min_attestors: u8,
    },
}

/// Outcome of a `ResolveDispute` call.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
    JsonSchema,
    UniversalWallet,
)]
pub enum DisputeDecision {
    /// Claim accepted; bond returned.
    Accept,
    /// Claim rejected; bond forfeited.
    Reject,
}

/// Reason a dispute was filed.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
    JsonSchema,
    UniversalWallet,
)]
pub enum DisputeGround {
    /// Claim does not match the bounty's acceptance predicate.
    AcceptanceMismatch,
    /// Payload looks fabricated or out-of-scope of the schema's declared shape.
    InvalidPayload,
    /// Attestor pool was rotated since the claim referenced it.
    StaleAttestorSet,
    /// Other.
    Other,
}

/// A single attestation claim inside a `ClaimBounty` batch.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
    JsonSchema,
    UniversalWallet,
)]
pub struct AttestationClaim {
    /// On-chain attestation id this claim references.
    pub attestation_id: AttestationId,
}

// ============================================================================
// Bounty + dispute state
// ============================================================================

/// Persistent bounty record stored under `Bounty::bounties`.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
#[serde(bound = "S::Address: Serialize + serde::de::DeserializeOwned")]
pub struct BountyState<S: Spec> {
    /// Bounty owner.
    pub poster: S::Address,
    /// Board schema this bounty composes against.
    pub board_schema_id: SchemaId,
    /// Total `AVOW` originally escrowed.
    pub pool: Amount,
    /// `AVOW` paid out per accepted claim.
    pub per_attestation: Amount,
    /// Filter rule for valid claims.
    pub acceptance: AcceptancePredicate,
    /// DA-layer block height the bounty expires at.
    pub expiry_da_height: u64,
    /// Dispute window in chain blocks after each claim.
    pub dispute_window_blocks: u32,
    /// Current lifecycle status.
    pub status: BountyStatus,
}

/// Dispute record stored under `Bounty::disputes`.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
#[serde(bound = "S::Address: Serialize + serde::de::DeserializeOwned")]
pub struct DisputeState<S: Spec> {
    /// Account that posted the bond.
    pub disputer: S::Address,
    /// Bond amount (equal to the contested claim's payout).
    pub bond: Amount,
    /// Why the dispute was filed.
    pub ground: DisputeGround,
}

/// Composite key for the dispute map. Carried as a struct rather than a
/// tuple so the StateMap key type has a stable name; the `Borsh` derive
/// gives it deterministic byte serialisation.
///
/// REST routes for `disputes` use the colon-joined `<bounty_id>:<attestation_id>`
/// form (e.g. `lbt1…:lat1…`) so callers compose two Bech32m ids into
/// one URL path segment. `FromStr` parses that form, `Display` emits it.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
pub struct DisputeKey {
    /// Bounty being disputed against.
    pub bounty_id: BountyId,
    /// Attestation whose claim is contested.
    pub attestation_id: AttestationId,
}

impl core::fmt::Display for DisputeKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}:{}", self.bounty_id.to_bech32(), self.attestation_id.to_bech32())
    }
}

impl core::str::FromStr for DisputeKey {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (bounty_str, attestation_str) = s.split_once(':').ok_or_else(|| {
            anyhow::anyhow!(
                "DisputeKey must be of the form `<bounty_id>:<attestation_id>` (got `{}`)",
                s
            )
        })?;
        let bounty_id = BountyId::from_str(bounty_str)
            .map_err(|e| anyhow::anyhow!("invalid bounty_id in DisputeKey: {}", e))?;
        let attestation_id = AttestationId::from_str(attestation_str)
            .map_err(|e| anyhow::anyhow!("invalid attestation_id in DisputeKey: {}", e))?;
        Ok(Self { bounty_id, attestation_id })
    }
}

// ============================================================================
// Module struct
// ============================================================================

/// The Ligate bounty marketplace module.
///
/// State shape matches RFC §"State". Escrow is tracked in
/// [`Bounty::escrow`] separate from the bank's view; the sum-invariant
/// `sum(escrow) == bank.balance(module_addr)` ships in the first
/// handler PR per RFC decision option B.
#[derive(Clone, ModuleInfo, ModuleRestApi)]
pub struct Bounty<S: Spec> {
    /// Module id, assigned by the SDK at runtime composition.
    #[id]
    pub id: ModuleId,

    /// All bounties, keyed by deterministic [`BountyId`].
    #[state]
    pub bounties: StateMap<BountyId, BountyState<S>>,

    /// Reverse index: open bounty ids by board schema.
    #[state]
    pub open_by_schema: StateMap<SchemaId, SafeVec<BountyId, MAX_OPEN_BOUNTIES_PER_SCHEMA>>,

    /// Per-bounty escrowed `AVOW`.
    #[state]
    pub escrow: StateMap<BountyId, Amount>,

    /// Active disputes keyed by `DisputeKey`.
    #[state]
    pub disputes: StateMap<DisputeKey, DisputeState<S>>,
}

// ============================================================================
// Call messages
// ============================================================================

/// Transactions this module accepts. v0 carries the wire shape; handler
/// bodies are no-ops (real logic lands in follow-up PRs against
/// [chain#519](https://github.com/ligate-io/ligate-chain/issues/519)).
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, UniversalWallet)]
#[sov_modules_api::macros::serialize(Borsh, Serde)]
#[schemars(bound = "S::Address: ::schemars::JsonSchema", rename = "BountyCallMessage")]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CallMessage<S: Spec> {
    /// Post a new bounty.
    PostBounty {
        /// Bounty board (schema) id.
        board_schema_id: SchemaId,
        /// Total `AVOW` to escrow.
        pool: Amount,
        /// Payout per accepted claim.
        per_attestation: Amount,
        /// Filter rule.
        acceptance: AcceptancePredicate,
        /// DA-layer expiry block height.
        expiry_da_height: u64,
        /// Chain blocks of dispute window after each claim.
        dispute_window_blocks: u32,
    },

    /// Claim one or more attestations against the bounty.
    ClaimBounty {
        /// Bounty being claimed against.
        bounty_id: BountyId,
        /// Batch of attestation claims.
        claims: SafeVec<AttestationClaim, MAX_CLAIMS_PER_CALL>,
    },

    /// Dispute one specific claim within the dispute window.
    DisputeAttestation {
        /// Bounty whose claim is being disputed.
        bounty_id: BountyId,
        /// Contested attestation.
        attestation_id: AttestationId,
        /// Reason.
        ground: DisputeGround,
    },

    /// Resolve an open dispute.
    ResolveDispute {
        /// Bounty whose dispute is being resolved.
        bounty_id: BountyId,
        /// Attestation whose claim is being resolved.
        attestation_id: AttestationId,
        /// Accept or reject.
        decision: DisputeDecision,
    },

    /// Cancel an unfunded or expired bounty.
    CancelBounty {
        /// Bounty to cancel.
        bounty_id: BountyId,
    },

    /// Phantom data carrier so the generic `S: Spec` parameter is used
    /// even when the visible variants do not reference `S::Address`
    /// directly. v0 only; future variants add S-typed fields and this
    /// can be removed (or kept as `#[non_exhaustive]` arms).
    #[doc(hidden)]
    #[schemars(skip)]
    _Phantom(core::marker::PhantomData<S>),
}

// ============================================================================
// Events
// ============================================================================

/// Typed events the module emits.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(bound = "S::Address: Serialize + serde::de::DeserializeOwned")]
#[schemars(bound = "S::Address: ::schemars::JsonSchema", rename = "BountyEvent")]
pub enum Event<S: Spec> {
    /// A new bounty was posted; escrow funded.
    BountyPosted {
        /// New bounty id.
        bounty_id: BountyId,
        /// Poster.
        poster: S::Address,
        /// Pool escrowed.
        pool: Amount,
    },
    /// An attestation was claimed against a bounty.
    BountyClaimed {
        /// Bounty paid out from.
        bounty_id: BountyId,
        /// Attestation that satisfied the claim.
        attestation_id: AttestationId,
        /// Amount paid out (net of board fee routing).
        payout: Amount,
        /// Payout recipient.
        attester: S::Address,
    },
    /// A claim was disputed; bond locked.
    BountyDisputed {
        /// Bounty whose claim is disputed.
        bounty_id: BountyId,
        /// Attestation under dispute.
        attestation_id: AttestationId,
        /// Disputer.
        disputer: S::Address,
        /// Bond amount.
        bond: Amount,
    },
    /// A dispute was resolved.
    DisputeResolved {
        /// Bounty whose dispute is resolved.
        bounty_id: BountyId,
        /// Attestation whose dispute is resolved.
        attestation_id: AttestationId,
        /// Final decision.
        decision: DisputeDecision,
        /// Recipient of the bond.
        bond_recipient: S::Address,
    },
    /// A bounty expired with refundable escrow.
    BountyExpired {
        /// Bounty that expired.
        bounty_id: BountyId,
        /// Amount returned to poster.
        refunded_to_poster: Amount,
    },
}

// ============================================================================
// Errors
// ============================================================================

/// Typed errors produced by the bounty module's handlers. v0 reserves a
/// `NotImplemented` variant so the handler signature is stable across
/// the skeleton-then-fill cycle.
#[derive(Debug, ThisError)]
#[non_exhaustive]
pub enum BountyError {
    /// v0 handlers are no-op stubs; full implementation lands in
    /// follow-up PRs against
    /// [chain#519](https://github.com/ligate-io/ligate-chain/issues/519).
    #[error("bounty module handler not yet implemented (chain#519 follow-up)")]
    NotImplemented,
}

// ============================================================================
// Genesis configuration
// ============================================================================

/// Bounty module genesis configuration. Empty at v0; no initial state.
#[derive(Clone, Debug, Default, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct BountyConfig {}

// ============================================================================
// Module impl
// ============================================================================

impl<S: Spec> Module for Bounty<S> {
    type Spec = S;
    type Config = BountyConfig;
    type CallMessage = CallMessage<S>;
    type Event = Event<S>;
    type Error = anyhow::Error;

    fn genesis(
        &mut self,
        _genesis_rollup_header: &<<S as Spec>::Da as DaSpec>::BlockHeader,
        _config: &Self::Config,
        _state: &mut impl GenesisState<S>,
    ) -> anyhow::Result<()> {
        // v0: empty genesis. Real per-board boards and pools get
        // registered at runtime via PostBounty txs, not at genesis.
        Ok(())
    }

    fn call(
        &mut self,
        msg: Self::CallMessage,
        context: &Context<Self::Spec>,
        state: &mut impl TxState<S>,
    ) -> Result<(), Self::Error> {
        match msg {
            CallMessage::PostBounty {
                board_schema_id,
                pool,
                per_attestation,
                acceptance,
                expiry_da_height,
                dispute_window_blocks,
            } => self.handle_post_bounty(
                board_schema_id,
                pool,
                per_attestation,
                acceptance,
                expiry_da_height,
                dispute_window_blocks,
                context,
                state,
            ),
            CallMessage::ClaimBounty { bounty_id, claims } => {
                self.handle_claim_bounty(bounty_id, claims, context, state)
            }
            CallMessage::DisputeAttestation { bounty_id, attestation_id, ground } => {
                self.handle_dispute_attestation(bounty_id, attestation_id, ground, context, state)
            }
            CallMessage::ResolveDispute { bounty_id, attestation_id, decision } => {
                self.handle_resolve_dispute(bounty_id, attestation_id, decision, context, state)
            }
            CallMessage::CancelBounty { bounty_id } => {
                self.handle_cancel_bounty(bounty_id, context, state)
            }
            CallMessage::_Phantom(_) => Ok(()),
        }
    }
}

// ============================================================================
// Handlers (no-op stubs, chain#519 follow-ups)
// ============================================================================

impl<S: Spec> Bounty<S> {
    /// v0 no-op stub. Escrow transfer + state-write logic lands in the
    /// first follow-up PR.
    #[allow(clippy::too_many_arguments)]
    fn handle_post_bounty(
        &mut self,
        _board_schema_id: SchemaId,
        _pool: Amount,
        _per_attestation: Amount,
        _acceptance: AcceptancePredicate,
        _expiry_da_height: u64,
        _dispute_window_blocks: u32,
        _context: &Context<S>,
        _state: &mut impl TxState<S>,
    ) -> anyhow::Result<()> {
        #[cfg(feature = "native")]
        metrics::record_post_bounty();
        Ok(())
    }

    /// v0 no-op stub. Payout + fee-routing-composition logic lands in
    /// the second follow-up PR.
    fn handle_claim_bounty(
        &mut self,
        _bounty_id: BountyId,
        _claims: SafeVec<AttestationClaim, MAX_CLAIMS_PER_CALL>,
        _context: &Context<S>,
        _state: &mut impl TxState<S>,
    ) -> anyhow::Result<()> {
        #[cfg(feature = "native")]
        metrics::record_claim_bounty();
        Ok(())
    }

    /// v0 no-op stub. Bond lock + dispute write lands in the third
    /// follow-up PR.
    fn handle_dispute_attestation(
        &mut self,
        _bounty_id: BountyId,
        _attestation_id: AttestationId,
        _ground: DisputeGround,
        _context: &Context<S>,
        _state: &mut impl TxState<S>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// v0 no-op stub. Bond payout + status update lands alongside
    /// [`Self::handle_dispute_attestation`].
    fn handle_resolve_dispute(
        &mut self,
        _bounty_id: BountyId,
        _attestation_id: AttestationId,
        _decision: DisputeDecision,
        _context: &Context<S>,
        _state: &mut impl TxState<S>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// v0 no-op stub. Refund + status update lands in the fourth
    /// follow-up PR.
    fn handle_cancel_bounty(
        &mut self,
        _bounty_id: BountyId,
        _context: &Context<S>,
        _state: &mut impl TxState<S>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}
