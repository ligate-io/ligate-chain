//! Ligate contract primitive, work-for-hire deliverable contracts.
//!
//! See [`docs/protocol/contract-primitive.md`][spec] for the full RFC.
//!
//! [spec]: https://github.com/ligate-io/ligate-chain/blob/main/docs/protocol/contract-primitive.md
//!
//! # v0 skeleton scope
//!
//! Module declarations, state shape, [`CallMessage`] variants, [`Event`]
//! variants exactly as the RFC describes. Handler bodies are no-op
//! stubs at v0; real state-transition logic lands in the follow-up
//! PR tracked under chain#536's sub-issues.
//!
//! Skeleton ships first because adding a module to the runtime
//! composition bumps `chain_hash` (new module shifts the borsh schema
//! fingerprint), and we want that change to ship cleanly through one
//! release. Handler PRs are zero-impact on chain identity afterwards.
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
use sov_bank::Amount;
use sov_modules_api::macros::UniversalWallet;
use sov_modules_api::{
    Context, DaSpec, GenesisState, Module, ModuleId, ModuleInfo, ModuleRestApi, Spec, StateMap,
    TxState,
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
    /// v0 handlers are no-op stubs; full implementation lands in
    /// follow-up PRs against chain#536.
    #[error("contract module handler not yet implemented (chain#536 follow-up)")]
    NotImplemented,
}

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
        _context: &Context<Self::Spec>,
        _state: &mut impl TxState<S>,
    ) -> Result<(), Self::Error> {
        match msg {
            CallMessage::PostContract { .. }
            | CallMessage::CommitToContract { .. }
            | CallMessage::DeliverContract { .. }
            | CallMessage::AcceptDelivery { .. }
            | CallMessage::RejectDelivery { .. }
            | CallMessage::ResolveContractDispute { .. }
            | CallMessage::CancelContract { .. } => {
                #[cfg(feature = "native")]
                metrics::record_contract_call();
                Ok(())
            }
            CallMessage::_Phantom(_) => Ok(()),
        }
    }
}
