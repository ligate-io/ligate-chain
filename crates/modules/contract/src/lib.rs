//! Ligate contract primitive, work-for-hire deliverable contracts.
//!
//! See [`docs/protocol/contract-primitive.md`][spec] for the full RFC.
//!
//! [spec]: https://github.com/ligate-io/ligate-chain/blob/main/docs/protocol/contract-primitive.md
//!
//! # v0 implementation scope
//!
//! Module declarations, state shape, [`CallMessage`] variants,
//! [`Event`] variants, AND handler logic for all seven call messages.
//!
//! Cross-module composition: `Contracts` references `attestation` (for
//! the `avow_token_id` read + delivery-attestation existence check)
//! and `sov-bank` (for escrow + payout transfers). Escrow lives at
//! the module-controlled address derived via `self.id.to_payable()`
//! (matches the bounty module + the SDK's `sov-sequencer-registry`
//! pattern). The chain-wide `AVOW` token id is read from the
//! attestation module's authoritative state at tx time, keeping this
//! module's wallet schema clean.
//!
//! # Module shape
//!
//! Five `StateMap`s mirroring the RFC's §"State" section, plus seven
//! [`CallMessage`] variants for the contract lifecycle. Contracts are
//! sibling to bounties, not anchored to schemas: the poster names a
//! specific arbiter at post time, and disputes flow through that
//! named address rather than through the bounty's "schema owner is
//! arbiter" model.
//!
//! | Entity | Identified by | Created by |
//! |---|---|---|
//! | [`ContractState`] | [`ContractId`] = `SHA-256(poster ‖ criteria_doc_hash ‖ nonce)` | [`CallMessage::PostContract`] |
//! | [`CommitRecord`] | `(ContractId, worker_address)` | [`CallMessage::CommitToContract`] |
//! | [`DeliveryRecord`] | [`ContractId`] | [`CallMessage::DeliverContract`] |
//! | [`DisputeRecord`] | [`ContractId`] | [`CallMessage::RejectDelivery`] |

#![deny(missing_docs)]

#[cfg(feature = "native")]
mod query;

#[cfg(feature = "native")]
pub use query::ContractResponse;

#[cfg(feature = "native")]
pub mod metrics;

use attestation::AttestationId;
use borsh::{BorshDeserialize, BorshSerialize};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sov_bank::{Amount, Coins, IntoPayable};
use sov_modules_api::macros::UniversalWallet;
use sov_modules_api::{
    Context, DaSpec, EventEmitter, GenesisState, Module, ModuleId, ModuleInfo, ModuleRestApi, Spec,
    StateMap, TxState,
};
use thiserror::Error as ThisError;

// ----- Identifier types -------------------------------------------------------

// `ContractId`: 32-byte hash with the `lct` Bech32m prefix. Follows
// the same shape as the attestation module's `lsc/las/lph/lpk/lat`
// family and the bounty module's `lbt`. Prefix tracked under chain#46.
mod contract_id_mod {
    sov_modules_api::impl_hash32_type!(ContractId, ContractIdBech32, "lct");
}
pub use contract_id_mod::{ContractId, ContractIdBech32};

impl ContractId {
    /// Compute the deterministic [`ContractId`] from the call inputs.
    ///
    /// `SHA-256(poster_bytes ‖ criteria_doc_hash ‖ nonce_le)`. The
    /// nonce is the poster's contract-module-local nonce at the time
    /// of `PostContract`; the same `(poster, criteria_doc_hash, nonce)`
    /// tuple is unique by construction.
    pub fn derive<A: AsRef<[u8]>>(poster: A, criteria_doc_hash: &[u8; 32], nonce: u64) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(poster.as_ref());
        hasher.update(criteria_doc_hash);
        hasher.update(nonce.to_le_bytes());
        let digest = hasher.finalize();
        let mut out = [0u8; 32];
        out.copy_from_slice(&digest);
        Self::from(out)
    }
}

// ----- Lifecycle status -------------------------------------------------------

/// Lifecycle status of a contract.
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
pub enum ContractStatus {
    /// Posted, accepting commits.
    Open,
    /// A worker has committed (and possibly delivered).
    Committed,
    /// Worker submitted delivery; awaiting acceptance window.
    Delivered,
    /// Poster accepted; payout settled.
    Accepted,
    /// Poster rejected; in dispute.
    Rejected,
    /// Arbiter is resolving.
    Disputed,
    /// Poster cancelled (only when Open + no commit).
    Cancelled,
    /// Expiry passed before delivery.
    Expired,
}

// ----- Dispute support types --------------------------------------------------

/// Reason a delivery was rejected and disputed.
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
    /// Delivery does not satisfy the criteria document.
    CriteriaMismatch,
    /// Delivery is malformed or unverifiable.
    MalformedDelivery,
    /// Delivery missed the expiry window.
    ExpiredAtDelivery,
    /// Generic ground; carry detail off-chain.
    Other,
}

/// Decision recorded on a `ResolveContractDispute` call.
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
    /// Arbiter sides with worker; pool flows to worker, bond returned.
    AcceptDelivery,
    /// Arbiter sides with poster; pool refunded, bond forfeited to poster.
    RejectDelivery,
}

// ----- Persistent records -----------------------------------------------------

/// Persistent contract record stored under [`Contracts::contracts`].
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
#[serde(bound = "S::Address: Serialize + serde::de::DeserializeOwned")]
pub struct ContractState<S: Spec> {
    /// Contract poster (buyer). Receives refunds on cancel and bond
    /// payouts on rejected dispute resolutions.
    pub poster: S::Address,
    /// Arbiter named at post time. Authorised to call
    /// [`CallMessage::ResolveContractDispute`].
    pub arbiter: S::Address,
    /// 32-byte content hash of the off-chain criteria document
    /// (test cases, acceptance rubric, etc.). Workers verify this
    /// hash matches the document they were given before committing.
    pub criteria_doc_hash: [u8; 32],
    /// Total `AVOW` originally escrowed.
    pub pool: Amount,
    /// DA-layer block height the contract expires at.
    pub expiry_da_height: u64,
    /// Window in chain blocks the poster has to accept-or-reject a
    /// delivery before it auto-accepts.
    pub dispute_window_blocks: u32,
    /// Arbiter fee in basis points (paid only if the arbiter resolves
    /// a dispute). Default 500 bps (5%); RFC §"Fee economics".
    pub arbiter_fee_bps: u16,
    /// Current lifecycle status.
    pub status: ContractStatus,
}

/// Commit record stored under [`Contracts::commits`], keyed by
/// `(ContractId, worker_address)`. Allows multiple workers to commit
/// to the same contract in parallel; the poster picks the winning
/// delivery via [`CallMessage::AcceptDelivery`].
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
#[serde(bound = "S::Address: Serialize + serde::de::DeserializeOwned")]
pub struct CommitRecord<S: Spec> {
    /// Worker address (echoed from the key for ergonomic state reads).
    pub worker: S::Address,
    /// SHA-256 commitment to the deliverable. Worker reveals the
    /// preimage when calling [`CallMessage::DeliverContract`].
    pub commit_hash: [u8; 32],
    /// Bond locked from the worker at commit time. Refunded on
    /// accepted delivery or expired-no-acceptance; forfeited on
    /// rejected-by-poster (resolved by arbiter against the worker).
    pub bond: Amount,
}

/// Delivery record stored under [`Contracts::deliveries`].
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
#[serde(bound = "S::Address: Serialize + serde::de::DeserializeOwned")]
pub struct DeliveryRecord<S: Spec> {
    /// Worker who delivered. Receives the pool on accepted delivery.
    pub worker: S::Address,
    /// Attestation id pointing at the on-chain proof of work (signed
    /// by the worker, optionally co-signed by reviewers / an eval
    /// pool). Composes with the attestation module for verification.
    pub deliverable_attestation_id: AttestationId,
    /// Chain block height the delivery landed at. Used to compute
    /// the auto-acceptance deadline (`delivered_at_block + window`).
    pub delivered_at_block: u64,
}

/// Dispute record stored under [`Contracts::disputes`], one per
/// contract at v0 (sequential disputes; v1 may add parallel dispute
/// support for multi-worker commits).
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
#[serde(bound = "S::Address: Serialize + serde::de::DeserializeOwned")]
pub struct DisputeRecord<S: Spec> {
    /// Worker whose delivery is disputed.
    pub worker: S::Address,
    /// Why the delivery was rejected.
    pub ground: DisputeGround,
}

// ----- Module struct ----------------------------------------------------------

/// The Ligate contract module.
#[derive(Clone, ModuleInfo, ModuleRestApi)]
pub struct Contracts<S: Spec> {
    /// Module id, assigned by the SDK at runtime composition.
    #[id]
    pub id: ModuleId,

    /// Reference to `sov-bank` for escrow deposits + payouts. Same
    /// pattern as the bounty module.
    #[module]
    pub bank: sov_bank::Bank<S>,

    /// Reference to the attestation module so handlers can verify a
    /// `deliverable_attestation_id` actually points at a valid
    /// attestation in module state.
    #[module]
    pub attestation: attestation::AttestationModule<S>,

    /// All contracts, keyed by deterministic [`ContractId`].
    #[state]
    pub contracts: StateMap<ContractId, ContractState<S>>,

    /// Commits per `(contract, worker)`. Allows multiple workers to
    /// commit to the same contract in parallel.
    #[state]
    pub commits: StateMap<CommitKey, CommitRecord<S>>,

    /// Delivery record per contract (only one delivery per contract
    /// at v0; the poster picks which committed worker delivers).
    #[state]
    pub deliveries: StateMap<ContractId, DeliveryRecord<S>>,

    /// Active disputes, one per contract at v0.
    #[state]
    pub disputes: StateMap<ContractId, DisputeRecord<S>>,

    /// Per-bounty escrowed `AVOW`. Decreases as payouts / refunds
    /// happen; matches the bounty module's escrow accounting shape.
    #[state]
    pub escrow: StateMap<ContractId, Amount>,

    /// Per-poster nonce counter for deterministic [`ContractId`]
    /// derivation. Same role as the bounty module's
    /// `next_poster_nonce`.
    #[state]
    pub next_poster_nonce: StateMap<S::Address, u64>,
}

/// Composite key for the [`Contracts::commits`] map. Carried as a
/// struct (rather than a tuple) so the StateMap key type has a
/// stable name; the colon-joined `<contract_id>:<worker_address>`
/// form is used in REST routes.
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
pub struct CommitKey {
    /// Contract being committed against.
    pub contract_id: ContractId,
    /// Worker who committed. Carried by raw bytes here so the
    /// struct is `Copy`; downstream code converts to `S::Address`
    /// via `S::Address::from(self.worker_bytes)`.
    pub worker_bytes: [u8; 28],
}

impl core::fmt::Display for CommitKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Hex-encoded worker bytes; the address Bech32m form requires
        // an `S::Address` parameterisation the struct can't carry
        // generically. The REST route handler decodes back with the
        // chain's `S::Address::from` at lookup time.
        write!(f, "{}:{}", self.contract_id.to_bech32(), hex::encode(self.worker_bytes))
    }
}

impl core::str::FromStr for CommitKey {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (cid_str, worker_hex) = s.split_once(':').ok_or_else(|| {
            anyhow::anyhow!("CommitKey must be `<contract_id>:<worker_hex>` (got `{}`)", s)
        })?;
        let contract_id = ContractId::from_str(cid_str)
            .map_err(|e| anyhow::anyhow!("invalid contract_id in CommitKey: {}", e))?;
        let worker_bytes_vec = hex::decode(worker_hex)
            .map_err(|e| anyhow::anyhow!("invalid worker hex in CommitKey: {}", e))?;
        if worker_bytes_vec.len() != 28 {
            anyhow::bail!("worker bytes must be 28 bytes, got {}", worker_bytes_vec.len());
        }
        let mut worker_bytes = [0u8; 28];
        worker_bytes.copy_from_slice(&worker_bytes_vec);
        Ok(Self { contract_id, worker_bytes })
    }
}

// ----- Call messages ----------------------------------------------------------

/// Transactions this module accepts.
///
/// v0 carries the wire shape; handler bodies are no-ops (real
/// state-transition logic lands in follow-up PRs against chain#536).
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, UniversalWallet)]
#[sov_modules_api::macros::serialize(Borsh, Serde)]
#[schemars(bound = "S::Address: ::schemars::JsonSchema", rename = "ContractCallMessage")]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CallMessage<S: Spec> {
    /// Post a new contract. Escrows `pool` `AVOW`.
    PostContract {
        /// Arbiter named at post time. Authorised to resolve disputes
        /// on this contract.
        arbiter: S::Address,
        /// 32-byte content hash of the off-chain criteria document.
        criteria_doc_hash: [u8; 32],
        /// Total `AVOW` to escrow.
        pool: Amount,
        /// DA-layer block height the contract expires at.
        expiry_da_height: u64,
        /// Window the poster has to accept-or-reject a delivery.
        dispute_window_blocks: u32,
        /// Arbiter fee in basis points; paid only if arbiter resolves.
        arbiter_fee_bps: u16,
    },

    /// Worker commits to delivering this contract. Bonds an amount
    /// that's refunded on accepted delivery or expired-no-acceptance,
    /// forfeited on rejected-by-poster + arbiter-confirmed.
    CommitToContract {
        /// Contract being committed against.
        contract_id: ContractId,
        /// SHA-256 of the deliverable (reveal preimage on
        /// [`CallMessage::DeliverContract`]).
        commit_hash: [u8; 32],
        /// Bond amount.
        bond: Amount,
    },

    /// Worker reveals + delivers their work. References an
    /// attestation that carries the proof-of-work.
    DeliverContract {
        /// Contract being delivered against.
        contract_id: ContractId,
        /// On-chain attestation id pointing at the delivery's proof.
        deliverable_attestation_id: AttestationId,
    },

    /// Poster accepts the delivery. Settles pool → worker, refunds
    /// the worker's bond.
    AcceptDelivery {
        /// Contract whose delivery is being accepted.
        contract_id: ContractId,
    },

    /// Poster rejects the delivery; transitions to Disputed.
    RejectDelivery {
        /// Contract whose delivery is being rejected.
        contract_id: ContractId,
        /// Reason.
        ground: DisputeGround,
    },

    /// Arbiter resolves an open dispute. Settles pool + bond per
    /// [`DisputeDecision`].
    ResolveContractDispute {
        /// Contract whose dispute is being resolved.
        contract_id: ContractId,
        /// Accept or reject.
        decision: DisputeDecision,
    },

    /// Poster cancels an unaccepted contract. Refunds escrow. Only
    /// callable when status is Open (no commits yet) or expired.
    CancelContract {
        /// Contract to cancel.
        contract_id: ContractId,
    },

    /// Phantom data carrier so the generic `S: Spec` parameter is
    /// used by the wire format even when no visible variant
    /// references `S::Address`. Mirrors the bounty module's
    /// `_Phantom` variant.
    #[doc(hidden)]
    #[schemars(skip)]
    _Phantom(core::marker::PhantomData<S>),
}

// ----- Events -----------------------------------------------------------------

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
#[schemars(bound = "S::Address: ::schemars::JsonSchema", rename = "ContractEvent")]
pub enum Event<S: Spec> {
    /// A new contract was posted; escrow funded.
    ContractPosted {
        /// New contract id.
        contract_id: ContractId,
        /// Poster.
        poster: S::Address,
        /// Arbiter named at post time.
        arbiter: S::Address,
        /// Pool escrowed.
        pool: Amount,
    },
    /// A worker committed to deliver.
    WorkerCommitted {
        /// Contract committed against.
        contract_id: ContractId,
        /// Worker.
        worker: S::Address,
        /// Commit hash.
        commit_hash: [u8; 32],
        /// Bond locked.
        bond: Amount,
    },
    /// A worker delivered.
    ContractDelivered {
        /// Contract delivered against.
        contract_id: ContractId,
        /// Worker.
        worker: S::Address,
        /// Attestation id of the deliverable.
        deliverable_attestation_id: AttestationId,
    },
    /// Poster accepted the delivery; pool flowed to worker.
    DeliveryAccepted {
        /// Contract whose delivery was accepted.
        contract_id: ContractId,
        /// Worker who got paid.
        worker: S::Address,
        /// Payout amount.
        payout: Amount,
    },
    /// Poster rejected the delivery; in dispute.
    DeliveryRejected {
        /// Contract whose delivery was rejected.
        contract_id: ContractId,
        /// Worker whose delivery was rejected.
        worker: S::Address,
        /// Reason.
        reason: DisputeGround,
    },
    /// Arbiter resolved a dispute.
    ContractDisputeResolved {
        /// Contract whose dispute was resolved.
        contract_id: ContractId,
        /// Decision.
        decision: DisputeDecision,
        /// Recipient of the pool.
        winner: S::Address,
    },
    /// Poster cancelled an open contract.
    ContractCancelled {
        /// Contract that was cancelled.
        contract_id: ContractId,
        /// Refunded amount.
        refunded_to_poster: Amount,
    },
    /// Contract expired without delivery.
    ContractExpired {
        /// Contract that expired.
        contract_id: ContractId,
        /// Refunded amount to poster.
        refunded_to_poster: Amount,
    },
}

// ----- Errors -----------------------------------------------------------------

/// Typed errors produced by the contract module's handlers.
#[derive(Debug, ThisError)]
#[non_exhaustive]
pub enum ContractError {
    /// `pool` was zero, which would post an unfunded contract.
    #[error("contract pool must be greater than zero")]
    EmptyPool,

    /// `expiry_da_height` was 0 (or otherwise inconsistent).
    #[error("expiry_da_height must be greater than zero")]
    InvalidExpiryHeight,

    /// `arbiter_fee_bps` exceeded the protocol cap (default 5000 = 50%).
    /// Defensive: griefing arbiters charging absurd fees should not
    /// be accepted at post time.
    #[error("arbiter_fee_bps ({bps}) exceeds protocol cap ({cap})")]
    ArbiterFeeExceedsCap {
        /// Submitted bps.
        bps: u16,
        /// Protocol cap (5000).
        cap: u16,
    },

    /// A [`ContractId`] collision was detected. Should be unreachable
    /// given the per-poster nonce.
    #[error("contract id collision (per-poster nonce invariant broken)")]
    DuplicateContract,

    /// Referenced contract id isn't stored.
    #[error("contract not found")]
    UnknownContract,

    /// Caller is not the contract's poster (for poster-only ops).
    #[error("not authorised: caller is not the contract poster")]
    NotPoster,

    /// Caller is not the contract's arbiter (for arbiter-only ops).
    #[error("not authorised: caller is not the contract arbiter")]
    NotArbiter,

    /// Contract status doesn't permit this op.
    #[error("contract status {status:?} not valid for this operation")]
    InvalidStatus {
        /// Current status.
        status: ContractStatus,
    },

    /// Worker bond was zero on `CommitToContract`. Even at v0 bonds
    /// need to be non-zero to make grief-disputes costly.
    #[error("commit bond must be greater than zero")]
    EmptyBond,

    /// Same `(contract_id, worker)` already has a commit.
    #[error("worker already committed to this contract")]
    DuplicateCommit,

    /// `DeliverContract` reached for a worker who hadn't committed.
    /// v0 requires commit-then-deliver to keep the flow disciplined.
    #[error("worker has not committed to this contract")]
    NoActiveCommit,

    /// `DeliverContract` referenced an attestation id that doesn't
    /// exist in the attestation module's store.
    #[error("attestation not found in attestation module")]
    UnknownAttestation,

    /// `DeliverContract` would overwrite an existing delivery. v0
    /// only accepts the first delivery per contract; later workers
    /// who committed but lost the race can `CancelContract` only via
    /// the poster's choice + bonds-refund flow.
    #[error("contract already has a delivery")]
    DuplicateDelivery,

    /// No delivery exists yet for an op that needs one (`Accept` /
    /// `Reject`).
    #[error("contract has no delivery to accept or reject")]
    NoDelivery,

    /// `RejectDelivery` reached for a contract that's not in
    /// [`ContractStatus::Delivered`].
    #[error("contract has no pending delivery to reject")]
    NoPendingDelivery,

    /// `ResolveContractDispute` reached for a contract that isn't in
    /// [`ContractStatus::Disputed`].
    #[error("contract is not in dispute")]
    NotInDispute,

    /// `CancelContract` reached when the contract has commits or
    /// delivery in flight. v0 only allows cancel from Open with no
    /// commits, or from Expired.
    #[error(
        "cannot cancel: contract has active commits or a delivery; \
         cancel only when no commits yet (or after expiry)"
    )]
    CancelWhileActive,

    /// Module pre-conditions not met. Should be unreachable after a
    /// clean `Module::genesis`.
    #[error(
        "contract module not initialised: cross-module attestation read failed for avow_token_id"
    )]
    ModuleNotInitialised,
}

/// Maximum `arbiter_fee_bps` accepted at post time. Same value as the
/// attestation module's `DEFAULT_MAX_BUILDER_BPS` (5000 = 50%).
const MAX_ARBITER_FEE_BPS: u16 = 5000;

// ----- Genesis configuration --------------------------------------------------

/// Contract module genesis configuration. Empty at v0; no initial
/// state is seeded at chain launch.
#[derive(Clone, Debug, Default, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct ContractConfig {}

// ----- Module impl ------------------------------------------------------------

impl<S: Spec> Module for Contracts<S> {
    type Spec = S;
    type Config = ContractConfig;
    type CallMessage = CallMessage<S>;
    type Event = Event<S>;
    type Error = anyhow::Error;

    fn genesis(
        &mut self,
        _genesis_rollup_header: &<<S as Spec>::Da as DaSpec>::BlockHeader,
        _config: &Self::Config,
        _state: &mut impl GenesisState<S>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn call(
        &mut self,
        msg: Self::CallMessage,
        context: &Context<Self::Spec>,
        state: &mut impl TxState<S>,
    ) -> Result<(), Self::Error> {
        let result = match msg {
            CallMessage::PostContract {
                arbiter,
                criteria_doc_hash,
                pool,
                expiry_da_height,
                dispute_window_blocks,
                arbiter_fee_bps,
            } => self.handle_post_contract(
                arbiter,
                criteria_doc_hash,
                pool,
                expiry_da_height,
                dispute_window_blocks,
                arbiter_fee_bps,
                context,
                state,
            ),
            CallMessage::CommitToContract { contract_id, commit_hash, bond } => {
                self.handle_commit_to_contract(contract_id, commit_hash, bond, context, state)
            }
            CallMessage::DeliverContract { contract_id, deliverable_attestation_id } => self
                .handle_deliver_contract(contract_id, deliverable_attestation_id, context, state),
            CallMessage::AcceptDelivery { contract_id } => {
                self.handle_accept_delivery(contract_id, context, state)
            }
            CallMessage::RejectDelivery { contract_id, ground } => {
                self.handle_reject_delivery(contract_id, ground, context, state)
            }
            CallMessage::ResolveContractDispute { contract_id, decision } => {
                self.handle_resolve_contract_dispute(contract_id, decision, context, state)
            }
            CallMessage::CancelContract { contract_id } => {
                self.handle_cancel_contract(contract_id, context, state)
            }
            CallMessage::_Phantom(_) => Ok(()),
        };

        #[cfg(feature = "native")]
        if result.is_ok() {
            metrics::record_contract_call();
        }
        result
    }
}

// ============================================================================
// Handlers
// ============================================================================

impl<S: Spec> Contracts<S> {
    /// Resolve the chain-wide `AVOW` token id at tx time by reading
    /// the attestation module's authoritative copy. Same pattern as
    /// the bounty module.
    fn read_avow_token_id(&self, state: &mut impl TxState<S>) -> anyhow::Result<sov_bank::TokenId> {
        self.attestation
            .avow_token_id
            .get(state)?
            .ok_or_else(|| ContractError::ModuleNotInitialised.into())
    }

    /// Atomically read + increment the per-poster nonce.
    fn bump_poster_nonce(
        &mut self,
        poster: &S::Address,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<u64> {
        let current = self.next_poster_nonce.get(poster, state)?.unwrap_or(0);
        self.next_poster_nonce.set(poster, &current.saturating_add(1), state)?;
        Ok(current)
    }

    /// Build the `CommitKey` for a `(contract, worker)` pair. The
    /// worker address is copied byte-for-byte; `S::Address`'s 28-byte
    /// representation matches the chain's `MultiAddress` shape.
    fn commit_key(contract_id: ContractId, worker: &S::Address) -> CommitKey {
        let mut worker_bytes = [0u8; 28];
        let addr_bytes = worker.as_ref();
        let len = core::cmp::min(addr_bytes.len(), 28);
        worker_bytes[..len].copy_from_slice(&addr_bytes[..len]);
        CommitKey { contract_id, worker_bytes }
    }

    /// Post a new contract. Validates inputs, derives a unique
    /// [`ContractId`], escrows `pool` `AVOW` from the poster into the
    /// module-controlled address, writes [`ContractState`], emits
    /// [`Event::ContractPosted`].
    #[allow(clippy::too_many_arguments)]
    fn handle_post_contract(
        &mut self,
        arbiter: S::Address,
        criteria_doc_hash: [u8; 32],
        pool: Amount,
        expiry_da_height: u64,
        dispute_window_blocks: u32,
        arbiter_fee_bps: u16,
        context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<()> {
        if pool == Amount::ZERO {
            return Err(ContractError::EmptyPool.into());
        }
        if expiry_da_height == 0 {
            return Err(ContractError::InvalidExpiryHeight.into());
        }
        if arbiter_fee_bps > MAX_ARBITER_FEE_BPS {
            return Err(ContractError::ArbiterFeeExceedsCap {
                bps: arbiter_fee_bps,
                cap: MAX_ARBITER_FEE_BPS,
            }
            .into());
        }

        let poster: S::Address = *context.sender();
        let nonce = self.bump_poster_nonce(&poster, state)?;
        let contract_id = ContractId::derive(poster.as_ref(), &criteria_doc_hash, nonce);
        if self.contracts.get(&contract_id, state)?.is_some() {
            return Err(ContractError::DuplicateContract.into());
        }

        let avow = self.read_avow_token_id(state)?;
        self.bank.transfer_from(
            &poster,
            self.id.clone().to_payable(),
            Coins { amount: pool, token_id: avow },
            state,
        )?;

        let contract_state = ContractState {
            poster,
            arbiter,
            criteria_doc_hash,
            pool,
            expiry_da_height,
            dispute_window_blocks,
            arbiter_fee_bps,
            status: ContractStatus::Open,
        };
        self.contracts.set(&contract_id, &contract_state, state)?;
        self.escrow.set(&contract_id, &pool, state)?;

        self.emit_event(state, Event::ContractPosted { contract_id, poster, arbiter, pool });
        Ok(())
    }

    /// Worker commits to delivering. Locks `bond` AVOW into the
    /// module escrow account. Allows multiple workers to commit in
    /// parallel (each (contract, worker) pair is a separate key).
    fn handle_commit_to_contract(
        &mut self,
        contract_id: ContractId,
        commit_hash: [u8; 32],
        bond: Amount,
        context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<()> {
        if bond == Amount::ZERO {
            return Err(ContractError::EmptyBond.into());
        }
        let mut contract =
            self.contracts.get(&contract_id, state)?.ok_or(ContractError::UnknownContract)?;
        if !matches!(contract.status, ContractStatus::Open | ContractStatus::Committed) {
            return Err(ContractError::InvalidStatus { status: contract.status }.into());
        }

        let worker: S::Address = *context.sender();
        let key = Self::commit_key(contract_id, &worker);
        if self.commits.get(&key, state)?.is_some() {
            return Err(ContractError::DuplicateCommit.into());
        }

        let avow = self.read_avow_token_id(state)?;
        self.bank.transfer_from(
            &worker,
            self.id.clone().to_payable(),
            Coins { amount: bond, token_id: avow },
            state,
        )?;

        self.commits.set(&key, &CommitRecord { worker, commit_hash, bond }, state)?;

        // Move status forward to Committed on the first commit.
        if matches!(contract.status, ContractStatus::Open) {
            contract.status = ContractStatus::Committed;
            self.contracts.set(&contract_id, &contract, state)?;
        }

        self.emit_event(state, Event::WorkerCommitted { contract_id, worker, commit_hash, bond });
        Ok(())
    }

    /// Worker delivers. Records the delivery + the attestation id
    /// pointing at proof-of-work. v0: only the first delivery is
    /// accepted per contract.
    fn handle_deliver_contract(
        &mut self,
        contract_id: ContractId,
        deliverable_attestation_id: AttestationId,
        context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<()> {
        let mut contract =
            self.contracts.get(&contract_id, state)?.ok_or(ContractError::UnknownContract)?;
        if !matches!(contract.status, ContractStatus::Committed) {
            return Err(ContractError::InvalidStatus { status: contract.status }.into());
        }
        if self.deliveries.get(&contract_id, state)?.is_some() {
            return Err(ContractError::DuplicateDelivery.into());
        }

        let worker: S::Address = *context.sender();
        let key = Self::commit_key(contract_id, &worker);
        if self.commits.get(&key, state)?.is_none() {
            return Err(ContractError::NoActiveCommit.into());
        }

        if self.attestation.attestations.get(&deliverable_attestation_id, state)?.is_none() {
            return Err(ContractError::UnknownAttestation.into());
        }

        let record = DeliveryRecord {
            worker,
            deliverable_attestation_id,
            // TODO: pull the current chain block height once the SDK
            // surfaces it to handlers. v0 pins 0; the timeout-based
            // auto-accept flow ships once block height is available
            // (chain#452 family).
            delivered_at_block: 0,
        };
        self.deliveries.set(&contract_id, &record, state)?;
        contract.status = ContractStatus::Delivered;
        self.contracts.set(&contract_id, &contract, state)?;

        self.emit_event(
            state,
            Event::ContractDelivered { contract_id, worker, deliverable_attestation_id },
        );
        Ok(())
    }

    /// Poster accepts the delivery. Pays the pool to the worker;
    /// refunds the worker's bond. Transitions to Accepted.
    fn handle_accept_delivery(
        &mut self,
        contract_id: ContractId,
        context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<()> {
        let mut contract =
            self.contracts.get(&contract_id, state)?.ok_or(ContractError::UnknownContract)?;
        let caller: S::Address = *context.sender();
        if caller != contract.poster {
            return Err(ContractError::NotPoster.into());
        }
        if !matches!(contract.status, ContractStatus::Delivered) {
            return Err(ContractError::InvalidStatus { status: contract.status }.into());
        }
        let delivery =
            self.deliveries.get(&contract_id, state)?.ok_or(ContractError::NoDelivery)?;
        let pool = contract.pool;
        let worker = delivery.worker;

        let avow = self.read_avow_token_id(state)?;
        // Payout: pool from module escrow → worker.
        self.bank.transfer_from(
            self.id.clone().to_payable(),
            &worker,
            Coins { amount: pool, token_id: avow },
            state,
        )?;
        self.escrow.set(&contract_id, &Amount::ZERO, state)?;

        // Refund the winning worker's bond.
        let key = Self::commit_key(contract_id, &worker);
        if let Some(commit) = self.commits.get(&key, state)? {
            self.bank.transfer_from(
                self.id.clone().to_payable(),
                &worker,
                Coins { amount: commit.bond, token_id: avow },
                state,
            )?;
            self.commits.remove(&key, state)?;
        }

        contract.status = ContractStatus::Accepted;
        self.contracts.set(&contract_id, &contract, state)?;

        self.emit_event(state, Event::DeliveryAccepted { contract_id, worker, payout: pool });
        Ok(())
    }

    /// Poster rejects the delivery. Writes a `DisputeRecord` and
    /// transitions to Disputed. Bond stays locked until arbiter
    /// resolves.
    fn handle_reject_delivery(
        &mut self,
        contract_id: ContractId,
        ground: DisputeGround,
        context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<()> {
        let mut contract =
            self.contracts.get(&contract_id, state)?.ok_or(ContractError::UnknownContract)?;
        let caller: S::Address = *context.sender();
        if caller != contract.poster {
            return Err(ContractError::NotPoster.into());
        }
        if !matches!(contract.status, ContractStatus::Delivered) {
            return Err(ContractError::NoPendingDelivery.into());
        }
        let delivery =
            self.deliveries.get(&contract_id, state)?.ok_or(ContractError::NoDelivery)?;

        self.disputes.set(
            &contract_id,
            &DisputeRecord { worker: delivery.worker, ground },
            state,
        )?;
        contract.status = ContractStatus::Disputed;
        self.contracts.set(&contract_id, &contract, state)?;

        self.emit_event(
            state,
            Event::DeliveryRejected { contract_id, worker: delivery.worker, reason: ground },
        );
        Ok(())
    }

    /// Arbiter resolves an open dispute. Pool + bond flow per
    /// `DisputeDecision`; arbiter earns `arbiter_fee_bps` slice of
    /// the pool. Transitions to Accepted / Rejected.
    fn handle_resolve_contract_dispute(
        &mut self,
        contract_id: ContractId,
        decision: DisputeDecision,
        context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<()> {
        let mut contract =
            self.contracts.get(&contract_id, state)?.ok_or(ContractError::UnknownContract)?;
        let caller: S::Address = *context.sender();
        if caller != contract.arbiter {
            return Err(ContractError::NotArbiter.into());
        }
        if !matches!(contract.status, ContractStatus::Disputed) {
            return Err(ContractError::NotInDispute.into());
        }
        let dispute = self.disputes.get(&contract_id, state)?.ok_or(ContractError::NotInDispute)?;

        let pool = contract.pool;
        let arbiter_cut =
            Amount::new(pool.0.saturating_mul(u128::from(contract.arbiter_fee_bps)) / 10_000);
        let pool_after_cut = Amount::new(pool.0.saturating_sub(arbiter_cut.0));
        let avow = self.read_avow_token_id(state)?;

        // Always pay the arbiter their cut from the escrow.
        if arbiter_cut > Amount::ZERO {
            self.bank.transfer_from(
                self.id.clone().to_payable(),
                &contract.arbiter,
                Coins { amount: arbiter_cut, token_id: avow },
                state,
            )?;
        }

        let worker = dispute.worker;
        let key = Self::commit_key(contract_id, &worker);
        let commit = self.commits.get(&key, state)?;

        let (winner, new_status) = match decision {
            DisputeDecision::AcceptDelivery => {
                // Worker wins: pool_after_cut → worker, bond refunded.
                if pool_after_cut > Amount::ZERO {
                    self.bank.transfer_from(
                        self.id.clone().to_payable(),
                        &worker,
                        Coins { amount: pool_after_cut, token_id: avow },
                        state,
                    )?;
                }
                if let Some(ref commit) = commit {
                    self.bank.transfer_from(
                        self.id.clone().to_payable(),
                        &worker,
                        Coins { amount: commit.bond, token_id: avow },
                        state,
                    )?;
                }
                (worker, ContractStatus::Accepted)
            }
            DisputeDecision::RejectDelivery => {
                // Poster wins: pool_after_cut refunded to poster,
                // bond forfeited to poster too.
                if pool_after_cut > Amount::ZERO {
                    self.bank.transfer_from(
                        self.id.clone().to_payable(),
                        &contract.poster,
                        Coins { amount: pool_after_cut, token_id: avow },
                        state,
                    )?;
                }
                if let Some(ref commit) = commit {
                    self.bank.transfer_from(
                        self.id.clone().to_payable(),
                        &contract.poster,
                        Coins { amount: commit.bond, token_id: avow },
                        state,
                    )?;
                }
                (contract.poster, ContractStatus::Rejected)
            }
        };

        self.escrow.set(&contract_id, &Amount::ZERO, state)?;
        if commit.is_some() {
            self.commits.remove(&key, state)?;
        }
        self.disputes.remove(&contract_id, state)?;

        contract.status = new_status;
        self.contracts.set(&contract_id, &contract, state)?;

        self.emit_event(state, Event::ContractDisputeResolved { contract_id, decision, winner });
        Ok(())
    }

    /// Poster cancels an Open contract (no commits) and refunds the
    /// escrow. v0 rejects the "cancel while commits in flight" case.
    fn handle_cancel_contract(
        &mut self,
        contract_id: ContractId,
        context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<()> {
        let mut contract =
            self.contracts.get(&contract_id, state)?.ok_or(ContractError::UnknownContract)?;
        let caller: S::Address = *context.sender();
        if caller != contract.poster {
            return Err(ContractError::NotPoster.into());
        }
        if matches!(
            contract.status,
            ContractStatus::Cancelled
                | ContractStatus::Accepted
                | ContractStatus::Rejected
                | ContractStatus::Expired
        ) {
            return Err(ContractError::InvalidStatus { status: contract.status }.into());
        }
        // v0: only cancel when status is Open (no commits yet). The
        // expired-with-commits case lands once handlers can read DA
        // height (chain#452 family).
        if !matches!(contract.status, ContractStatus::Open) {
            return Err(ContractError::CancelWhileActive.into());
        }

        let escrow_remaining = self.escrow.get(&contract_id, state)?.unwrap_or(Amount::ZERO);
        if escrow_remaining > Amount::ZERO {
            let avow = self.read_avow_token_id(state)?;
            self.bank.transfer_from(
                self.id.clone().to_payable(),
                &contract.poster,
                Coins { amount: escrow_remaining, token_id: avow },
                state,
            )?;
            self.escrow.set(&contract_id, &Amount::ZERO, state)?;
        }
        contract.status = ContractStatus::Cancelled;
        self.contracts.set(&contract_id, &contract, state)?;

        self.emit_event(
            state,
            Event::ContractCancelled { contract_id, refunded_to_poster: escrow_remaining },
        );
        Ok(())
    }
}
