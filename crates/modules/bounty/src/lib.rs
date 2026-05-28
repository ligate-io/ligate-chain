//! Ligate bounty marketplace, on-chain bounty primitive composed on the
//! [`attestation`] module.
//!
//! See [`docs/protocol/bounty-marketplace.md`][spec] for the full RFC.
//!
//! [spec]: https://github.com/ligate-io/ligate-chain/blob/main/docs/protocol/bounty-marketplace.md
//!
//! # v0 implementation scope
//!
//! This crate ships the module declarations, state shape, [`CallMessage`]
//! variants, [`Event`] variants, and full handler logic for all five call
//! messages per the RFC.
//!
//! Cross-module composition: `Bounty` references `attestation` (for board
//! schema and attestation reads on `ClaimBounty`) and `sov-bank` (for
//! escrow + payout transfers). Escrow lives at the module-controlled
//! address derived via `self.id.to_payable()` (the SDK pattern used by
//! `sov-sequencer-registry` for staked collateral); the chain-wide
//! `AVOW` token id is read from the attestation module's authoritative
//! state at tx time, keeping this module's wallet schema clean.
//!
//! # Module shape
//!
//! Five `StateMap`s mirroring the RFC's "State" section plus a
//! per-poster nonce counter, five [`CallMessage`] variants for the
//! lifecycle. Bounty boards are [`attestation::Schema`]s; this module's
//! state references their [`SchemaId`] but does not duplicate schema
//! data.

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
use sov_bank::{Amount, Coins, IntoPayable};
use sov_modules_api::macros::UniversalWallet;
use sov_modules_api::{
    Context, DaSpec, EventEmitter, GenesisState, Module, ModuleId, ModuleInfo, ModuleRestApi,
    SafeVec, Spec, StateMap, TxState,
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
/// `sum(escrow) == bank.balance(module_addr)` is asserted in test
/// scaffolding (production-tier runtime check is a follow-up).
#[derive(Clone, ModuleInfo, ModuleRestApi)]
pub struct Bounty<S: Spec> {
    /// Module id, assigned by the SDK at runtime composition.
    #[id]
    pub id: ModuleId,

    /// Reference to the `sov-bank` module. Used for escrow deposits at
    /// [`CallMessage::PostBounty`] time, payouts at
    /// [`CallMessage::ClaimBounty`] time, bond locks at
    /// [`CallMessage::DisputeAttestation`] time, bond payouts at
    /// [`CallMessage::ResolveDispute`] time, and refunds at
    /// [`CallMessage::CancelBounty`] time.
    #[module]
    pub bank: sov_bank::Bank<S>,

    /// Reference to the [`attestation`] module. Used to validate that
    /// the board schema referenced by a posted bounty actually exists
    /// (`PostBounty`), and to read attestation records when claims
    /// reference them (`ClaimBounty`).
    #[module]
    pub attestation: attestation::AttestationModule<S>,

    /// All bounties, keyed by deterministic [`BountyId`].
    #[state]
    pub bounties: StateMap<BountyId, BountyState<S>>,

    /// Reverse index: open bounty ids by board schema. Lets the
    /// indexer matching service fetch "what's open on this schema"
    /// in a single chain call.
    #[state]
    pub open_by_schema: StateMap<SchemaId, SafeVec<BountyId, MAX_OPEN_BOUNTIES_PER_SCHEMA>>,

    /// Per-bounty escrowed `AVOW`. Decreases as payouts happen on
    /// [`CallMessage::ClaimBounty`]; refunded on
    /// [`CallMessage::CancelBounty`].
    #[state]
    pub escrow: StateMap<BountyId, Amount>,

    /// Active disputes keyed by [`DisputeKey`].
    #[state]
    pub disputes: StateMap<DisputeKey, DisputeState<S>>,

    /// Per-poster nonce counter. Incremented on each
    /// [`CallMessage::PostBounty`] for the given sender; combined with
    /// `(poster, board_schema_id)` to derive a unique [`BountyId`].
    /// Using a module-local nonce (rather than the SDK's per-account
    /// `Accounts::nonce`) keeps the bounty id derivation independent
    /// of any module reordering and survives module-only state
    /// snapshots.
    #[state]
    pub next_poster_nonce: StateMap<S::Address, u64>,
}

// Module-controlled escrow address is derived at tx time from
// `self.id.clone().to_payable()` (the SDK pattern used by
// `sov-sequencer-registry` for staked-AVOW collateral). The address
// is deterministic from `BOUNTY_DISCRIMINANT = 22`, so no genesis
// bootstrap is required and no state field is needed to remember it.
//
// `avow_token_id` is read at tx time from the attestation module's
// own `avow_token_id` state value via the `#[module] attestation`
// composition above; this keeps the chain-wide AVOW configuration
// authoritative in one place and avoids any wallet-schema bump from
// adding the field to `BountyConfig`.

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

/// Typed errors produced by the bounty module's handlers.
#[derive(Debug, ThisError)]
#[non_exhaustive]
pub enum BountyError {
    /// `pool` was zero, which would post an unfunded bounty.
    #[error("bounty pool must be greater than zero")]
    EmptyPool,

    /// `per_attestation` was zero, which would award no payout per claim.
    #[error("per_attestation payout must be greater than zero")]
    ZeroPerAttestation,

    /// `per_attestation > pool`, which means the bounty cannot pay out
    /// a single claim. Rejected at post time so misconfigured bounties
    /// never reach the matching surface.
    #[error("per_attestation ({per_attestation}) exceeds pool ({pool})")]
    PerAttestationExceedsPool {
        /// Submitted per_attestation amount.
        per_attestation: u128,
        /// Submitted pool amount.
        pool: u128,
    },

    /// `expiry_da_height` was set to 0 (or otherwise inconsistent with
    /// the chain's current DA height view). v0 only rejects 0; richer
    /// "expiry in the past" checks land once the runtime exposes the
    /// current DA height to handlers (#452 family).
    #[error("expiry_da_height must be greater than zero")]
    InvalidExpiryHeight,

    /// `PostBounty` referenced a schema id that isn't registered on
    /// the attestation module.
    #[error("board schema not found in attestation module")]
    UnknownBoardSchema,

    /// A [`BountyId`] collision was detected. Should be unreachable
    /// given the per-poster nonce, but surfaced as an explicit error
    /// to keep state-transition code honest.
    #[error("bounty id collision (per-poster nonce invariant broken)")]
    DuplicateBounty,

    /// The reverse index `open_by_schema` is at the
    /// [`MAX_OPEN_BOUNTIES_PER_SCHEMA`] cap for this schema; reject
    /// new posts against the schema until claims clear or bounties
    /// expire. Bound is generous (1024); hitting it indicates either
    /// genuine board saturation or an indexer crawl that needs
    /// rate-limiting.
    #[error("open bounties on this schema at cap ({MAX_OPEN_BOUNTIES_PER_SCHEMA})")]
    TooManyOpenBounties,

    /// Tx referenced a [`BountyId`] that isn't stored.
    #[error("bounty not found")]
    UnknownBounty,

    /// Tx is gated to the bounty poster (e.g. `CancelBounty`) and the
    /// sender isn't that address.
    #[error("not authorised: caller is not the bounty poster")]
    NotPoster,

    /// `ResolveDispute` is gated to the board operator (the
    /// `attestation::Schema::owner` of the board) and the sender
    /// isn't that address.
    #[error("not authorised: caller is not the board operator")]
    NotBoardOperator,

    /// Bounty is not in the [`BountyStatus::Open`] (or
    /// [`BountyStatus::Exhausted`] for disputes) state required by
    /// the called handler. Rejection signals "this tx came in too
    /// late or too early for the bounty's lifecycle stage."
    #[error("bounty status {status:?} not valid for this operation")]
    InvalidStatus {
        /// Bounty's current status.
        status: BountyStatus,
    },

    /// A `ClaimBounty` referenced an [`AttestationId`] that doesn't
    /// exist in the attestation module's store.
    #[error("attestation not found in attestation module")]
    UnknownAttestation,

    /// A `ClaimBounty` referenced an attestation whose schema id
    /// differs from the bounty's `board_schema_id`. Off-by-one bug
    /// catcher: the bounty pays for evidence on schema A, the claimant
    /// submitted an attestation on schema B, the chain rejects.
    #[error("claim references wrong-schema attestation (expected {expected}, got {actual})")]
    SchemaMismatch {
        /// Schema id the bounty pays for.
        expected: SchemaId,
        /// Schema id of the referenced attestation.
        actual: SchemaId,
    },

    /// A `ClaimBounty` referenced an attestation that doesn't satisfy
    /// the bounty's [`AcceptancePredicate`].
    #[error("claim does not satisfy bounty acceptance predicate")]
    AcceptanceMismatch,

    /// `ClaimBounty` would drain escrow below `per_attestation` for at
    /// least one claim in the batch; the chain rejects the whole
    /// batch (no partial settlement at v0).
    #[error("escrow insufficient for batch (escrow={escrow}, would_pay={would_pay})")]
    EscrowInsufficient {
        /// Current escrow balance.
        escrow: u128,
        /// Sum the batch would pay if accepted.
        would_pay: u128,
    },

    /// `ClaimBounty` batch was empty.
    #[error("claim batch must contain at least one claim")]
    EmptyClaimBatch,

    /// `DisputeAttestation` referenced a `(bounty_id, attestation_id)`
    /// pair that already has an active dispute. v0 allows one dispute
    /// per claim; serial-disputer patterns are a v2 disputes-module
    /// concern.
    #[error("dispute already active for this claim")]
    DuplicateDispute,

    /// `ResolveDispute` referenced a dispute key that isn't stored.
    #[error("dispute not found")]
    UnknownDispute,

    /// `CancelBounty` was called before expiry on a bounty that
    /// already has claims paid out. v0 only allows cancel when
    /// expired OR when no claims have been made (pool intact).
    #[error("cannot cancel: bounty has been claimed and is not yet expired")]
    CancelWhileClaimed,

    /// Module pre-conditions not met. Should be unreachable after a
    /// clean `Module::genesis`, but surfaced as an explicit error so
    /// handlers fail honestly instead of panicking.
    #[error(
        "bounty module not initialised: cross-module attestation read failed for avow_token_id"
    )]
    ModuleNotInitialised,
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
// Handlers
// ============================================================================

impl<S: Spec> Bounty<S> {
    /// Resolve the chain-wide `AVOW` token id at tx time by reading the
    /// attestation module's authoritative copy.
    fn read_avow_token_id(&self, state: &mut impl TxState<S>) -> anyhow::Result<sov_bank::TokenId> {
        self.attestation
            .avow_token_id
            .get(state)?
            .ok_or_else(|| BountyError::ModuleNotInitialised.into())
    }

    /// Atomically read + increment the per-poster nonce. Returns the
    /// pre-increment value for the caller to pass to
    /// [`BountyId::derive`].
    fn bump_poster_nonce(
        &mut self,
        poster: &S::Address,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<u64> {
        let current = self.next_poster_nonce.get(poster, state)?.unwrap_or(0);
        self.next_poster_nonce.set(poster, &current.saturating_add(1), state)?;
        Ok(current)
    }

    /// Decide whether an attestation satisfies a bounty's acceptance
    /// predicate. v0 conservatively passes `AttestorSet` (the schema's
    /// attestor_set is enforced at attestation-submission time), and
    /// uses the supplied `peer_count` for `PeerCount` (counts come
    /// from a follow-up indexer-side enrichment; v0 passes 1).
    fn matches_acceptance(
        attestation_record: &attestation::Attestation<S>,
        bounty_schema_id: SchemaId,
        predicate: &AcceptancePredicate,
        peer_count: u8,
    ) -> bool {
        if attestation_record.schema_id != bounty_schema_id {
            return false;
        }
        match predicate {
            AcceptancePredicate::Any => true,
            AcceptancePredicate::AttestorSet(_) => true,
            AcceptancePredicate::PayloadHashes(hashes) => {
                hashes.iter().any(|h| h == &attestation_record.payload_hash)
            }
            AcceptancePredicate::PeerCount { min_attestors } => peer_count >= *min_attestors,
        }
    }

    /// Post a new bounty. Validates the inputs, derives a unique
    /// [`BountyId`] from `(sender, board_schema_id, per-poster nonce)`,
    /// escrows `pool` `AVOW` from the sender into the bounty module's
    /// own address (`self.id.to_payable()`), writes the
    /// [`BountyState`] under [`BountyStatus::Open`], updates the
    /// reverse `open_by_schema` index, and emits
    /// [`Event::BountyPosted`].
    #[allow(clippy::too_many_arguments)]
    fn handle_post_bounty(
        &mut self,
        board_schema_id: SchemaId,
        pool: Amount,
        per_attestation: Amount,
        acceptance: AcceptancePredicate,
        expiry_da_height: u64,
        dispute_window_blocks: u32,
        context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<()> {
        // 1. Validate inputs.
        if pool == Amount::ZERO {
            return Err(BountyError::EmptyPool.into());
        }
        if per_attestation == Amount::ZERO {
            return Err(BountyError::ZeroPerAttestation.into());
        }
        if per_attestation > pool {
            return Err(BountyError::PerAttestationExceedsPool {
                per_attestation: per_attestation.0,
                pool: pool.0,
            }
            .into());
        }
        if expiry_da_height == 0 {
            return Err(BountyError::InvalidExpiryHeight.into());
        }

        // 2. Verify the board schema is registered.
        if self.attestation.schemas.get(&board_schema_id, state)?.is_none() {
            return Err(BountyError::UnknownBoardSchema.into());
        }

        // 3. Derive a unique BountyId via the per-poster nonce.
        let poster: S::Address = *context.sender();
        let nonce = self.bump_poster_nonce(&poster, state)?;
        let bounty_id = BountyId::derive(poster.as_ref(), &board_schema_id, nonce);
        if self.bounties.get(&bounty_id, state)?.is_some() {
            return Err(BountyError::DuplicateBounty.into());
        }

        // 4. Move `pool` AVOW from poster to the module address.
        let avow = self.read_avow_token_id(state)?;
        self.bank.transfer_from(
            &poster,
            self.id.clone().to_payable(),
            Coins { amount: pool, token_id: avow },
            state,
        )?;

        // 5. Persist the bounty state + escrow accounting.
        let bounty_state = BountyState {
            poster,
            board_schema_id,
            pool,
            per_attestation,
            acceptance,
            expiry_da_height,
            dispute_window_blocks,
            status: BountyStatus::Open,
        };
        self.bounties.set(&bounty_id, &bounty_state, state)?;
        self.escrow.set(&bounty_id, &pool, state)?;

        // 6. Update the reverse index used by the matching service.
        let mut open_vec: Vec<BountyId> = self
            .open_by_schema
            .get(&board_schema_id, state)?
            .map(|v| v.into_iter().collect())
            .unwrap_or_default();
        if open_vec.len() >= MAX_OPEN_BOUNTIES_PER_SCHEMA {
            return Err(BountyError::TooManyOpenBounties.into());
        }
        open_vec.push(bounty_id);
        let safe: SafeVec<BountyId, MAX_OPEN_BOUNTIES_PER_SCHEMA> =
            SafeVec::try_from(open_vec).map_err(|_| BountyError::TooManyOpenBounties)?;
        self.open_by_schema.set(&board_schema_id, &safe, state)?;

        // 7. Emit + count.
        self.emit_event(state, Event::BountyPosted { bounty_id, poster, pool });
        #[cfg(feature = "native")]
        metrics::record_post_bounty();
        Ok(())
    }

    /// Settle a batch of attestation claims against an open bounty.
    /// Pre-validates each claim (schema match + acceptance predicate +
    /// escrow sufficiency for the whole batch), then transfers
    /// `per_attestation` `AVOW` from the module escrow to each
    /// attestation's submitter and emits [`Event::BountyClaimed`].
    fn handle_claim_bounty(
        &mut self,
        bounty_id: BountyId,
        claims: SafeVec<AttestationClaim, MAX_CLAIMS_PER_CALL>,
        _context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<()> {
        let claims_vec: Vec<AttestationClaim> = claims.into_iter().collect();
        if claims_vec.is_empty() {
            return Err(BountyError::EmptyClaimBatch.into());
        }

        let mut bounty = self.bounties.get(&bounty_id, state)?.ok_or(BountyError::UnknownBounty)?;
        if bounty.status != BountyStatus::Open {
            return Err(BountyError::InvalidStatus { status: bounty.status }.into());
        }

        // Pre-flight: confirm every claim matches the bounty and the
        // batch's total payout fits in the current escrow. Single
        // failure rejects the whole batch.
        let mut payouts: Vec<(AttestationId, S::Address)> = Vec::with_capacity(claims_vec.len());
        let per_attestation = bounty.per_attestation;
        for claim in &claims_vec {
            let att = self
                .attestation
                .attestations
                .get(&claim.attestation_id, state)?
                .ok_or(BountyError::UnknownAttestation)?;
            if att.schema_id != bounty.board_schema_id {
                return Err(BountyError::SchemaMismatch {
                    expected: bounty.board_schema_id,
                    actual: att.schema_id,
                }
                .into());
            }
            if !Self::matches_acceptance(&att, bounty.board_schema_id, &bounty.acceptance, 1) {
                return Err(BountyError::AcceptanceMismatch.into());
            }
            payouts.push((claim.attestation_id, att.submitter));
        }

        let would_pay: u128 = per_attestation.0.saturating_mul(payouts.len() as u128);
        let escrow_now = self.escrow.get(&bounty_id, state)?.unwrap_or(Amount::ZERO);
        if would_pay > escrow_now.0 {
            return Err(BountyError::EscrowInsufficient { escrow: escrow_now.0, would_pay }.into());
        }

        // Pay out each claim.
        let avow = self.read_avow_token_id(state)?;
        for (attestation_id, attester) in &payouts {
            self.bank.transfer_from(
                self.id.clone().to_payable(),
                attester,
                Coins { amount: per_attestation, token_id: avow },
                state,
            )?;
            self.emit_event(
                state,
                Event::BountyClaimed {
                    bounty_id,
                    attestation_id: *attestation_id,
                    payout: per_attestation,
                    attester: *attester,
                },
            );
        }

        // Update escrow + status.
        let new_escrow = Amount::new(escrow_now.0.saturating_sub(would_pay));
        self.escrow.set(&bounty_id, &new_escrow, state)?;
        if new_escrow.0 < per_attestation.0 {
            bounty.status = BountyStatus::Exhausted;
            self.bounties.set(&bounty_id, &bounty, state)?;
        }

        #[cfg(feature = "native")]
        metrics::record_claim_bounty();
        Ok(())
    }

    /// Open a dispute against a previously-claimed attestation. Locks
    /// a bond equal to the bounty's `per_attestation` from the
    /// disputer into the module escrow; writes the [`DisputeState`];
    /// emits [`Event::BountyDisputed`].
    fn handle_dispute_attestation(
        &mut self,
        bounty_id: BountyId,
        attestation_id: AttestationId,
        ground: DisputeGround,
        context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<()> {
        let bounty = self.bounties.get(&bounty_id, state)?.ok_or(BountyError::UnknownBounty)?;
        let key = DisputeKey { bounty_id, attestation_id };
        if self.disputes.get(&key, state)?.is_some() {
            return Err(BountyError::DuplicateDispute.into());
        }
        let disputer: S::Address = *context.sender();
        let bond = bounty.per_attestation;

        let avow = self.read_avow_token_id(state)?;
        self.bank.transfer_from(
            &disputer,
            self.id.clone().to_payable(),
            Coins { amount: bond, token_id: avow },
            state,
        )?;
        self.disputes.set(&key, &DisputeState { disputer, bond, ground }, state)?;
        self.emit_event(state, Event::BountyDisputed { bounty_id, attestation_id, disputer, bond });
        Ok(())
    }

    /// Resolve an open dispute. Caller must be the board operator
    /// (`attestation::Schema::owner` of the bounty's board schema).
    /// Bond flows to the winner per [`DisputeDecision`].
    fn handle_resolve_dispute(
        &mut self,
        bounty_id: BountyId,
        attestation_id: AttestationId,
        decision: DisputeDecision,
        context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<()> {
        let bounty = self.bounties.get(&bounty_id, state)?.ok_or(BountyError::UnknownBounty)?;
        let board_schema = self
            .attestation
            .schemas
            .get(&bounty.board_schema_id, state)?
            .ok_or(BountyError::UnknownBoardSchema)?;
        let caller: S::Address = *context.sender();
        if caller != board_schema.owner {
            return Err(BountyError::NotBoardOperator.into());
        }
        let key = DisputeKey { bounty_id, attestation_id };
        let dispute = self.disputes.get(&key, state)?.ok_or(BountyError::UnknownDispute)?;

        // Bond goes to the winner: accept → disputer (the dispute was
        // upheld), reject → bounty poster (the claim was valid; the
        // disputer was wrong and forfeits the bond into the bounty
        // pool's owner).
        let recipient = match decision {
            DisputeDecision::Accept => dispute.disputer,
            DisputeDecision::Reject => bounty.poster,
        };
        let avow = self.read_avow_token_id(state)?;
        self.bank.transfer_from(
            self.id.clone().to_payable(),
            &recipient,
            Coins { amount: dispute.bond, token_id: avow },
            state,
        )?;
        self.disputes.remove(&key, state)?;
        self.emit_event(
            state,
            Event::DisputeResolved {
                bounty_id,
                attestation_id,
                decision,
                bond_recipient: recipient,
            },
        );
        Ok(())
    }

    /// Cancel a bounty and refund remaining escrow to the poster.
    /// v0 only allows cancellation when no claims have been paid out
    /// (escrow still equals pool). The "expired-with-claims" case
    /// lands once handlers can read the current DA height.
    fn handle_cancel_bounty(
        &mut self,
        bounty_id: BountyId,
        context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<()> {
        let mut bounty = self.bounties.get(&bounty_id, state)?.ok_or(BountyError::UnknownBounty)?;
        let caller: S::Address = *context.sender();
        if caller != bounty.poster {
            return Err(BountyError::NotPoster.into());
        }
        if matches!(bounty.status, BountyStatus::Cancelled | BountyStatus::Finalised) {
            return Err(BountyError::InvalidStatus { status: bounty.status }.into());
        }

        let escrow_remaining = self.escrow.get(&bounty_id, state)?.unwrap_or(Amount::ZERO);
        let no_claims = escrow_remaining == bounty.pool;
        if !no_claims {
            return Err(BountyError::CancelWhileClaimed.into());
        }

        if escrow_remaining > Amount::ZERO {
            let avow = self.read_avow_token_id(state)?;
            self.bank.transfer_from(
                self.id.clone().to_payable(),
                &bounty.poster,
                Coins { amount: escrow_remaining, token_id: avow },
                state,
            )?;
            self.escrow.set(&bounty_id, &Amount::ZERO, state)?;
        }
        bounty.status = BountyStatus::Cancelled;
        self.bounties.set(&bounty_id, &bounty, state)?;

        // Drop the open-index entry so the matching service stops
        // surfacing the cancelled bounty.
        if let Some(open_list) = self.open_by_schema.get(&bounty.board_schema_id, state)? {
            let filtered: Vec<BountyId> =
                open_list.into_iter().filter(|id| *id != bounty_id).collect();
            let safe: SafeVec<BountyId, MAX_OPEN_BOUNTIES_PER_SCHEMA> =
                SafeVec::try_from(filtered).expect("filtered subset fits the original cap");
            self.open_by_schema.set(&bounty.board_schema_id, &safe, state)?;
        }

        self.emit_event(
            state,
            Event::BountyExpired { bounty_id, refunded_to_poster: escrow_remaining },
        );
        Ok(())
    }
}
