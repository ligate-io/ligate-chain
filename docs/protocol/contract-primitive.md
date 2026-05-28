# Contract primitive (RFC v0.1)

**Status:** design draft. Proposes a new chain module sibling to the bounty marketplace ([`bounty-marketplace.md`](bounty-marketplace.md), chain#519). Tracked under chain#385 (the bounty work umbrella) until a dedicated tracking issue is filed alongside this RFC's merge.

This RFC argues that Bounty and Contract are different economic primitives that should be different chain primitives, then sketches the Contract shape and how it composes with the existing attestation + bounty + dispute machinery.

## The conflation we're undoing

The v0 bounty marketplace ([chain#519](https://github.com/ligate-io/ligate-chain/issues/519)) ships one primitive (Bounty) that is best fit for **open calls for fungible evidence**: 10,000 paragraphs attested as original, 1,000 medical records attested as anonymised, 100 KYC checks attested as compliant. Many workers each contribute one of N; the chain pays per accepted attestation; everyone earns.

It is NOT a clean fit for **specific deliverable contracts**: "I need a prompt for a Python sentiment-analysis bot, 100 AVOW for the winning submission." The buyer wants the deliverable (the prompt), not just attestations of "a prompt exists that meets criteria." The prompt-evaluation-pool RFC ([`prompt-evaluation-pool.md`](prompt-evaluation-pool.md)) showed you can hack the work-for-hire shape into the Bounty primitive via a reusable evaluation schema, but the hack has friction:

- One-off bounties need a schema to anchor against, even if the schema only ever serves that one bounty.
- Schema authors capture `fee_routing_bps` on bounties they don't add ongoing value to.
- Discovery is awkward: "what work-for-hire contracts are open for prompt engineering" doesn't compose against the schema-indexed bounty surface.
- The chain's mental model is "everything is composed of attestations + schemas + bounties," which is forced when the work isn't naturally schema-shaped.

The cleaner long-term answer is to add a Contract primitive alongside Bounty. They serve different needs; they should be different primitives.

## When to use which

| Use case | Primitive |
|----------|-----------|
| 10,000 attested-original paragraphs at 0.10 AVOW each | Bounty |
| 1,000 KYC checks attested as compliant at 5 AVOW each | Bounty |
| 100 medical records attested as anonymised at 1 AVOW each | Bounty |
| A prompt for a specific Python bot, 100 AVOW for the winning submission | Contract |
| A landing page design at $5,000, evaluated by a specific arbitrator | Contract |
| A research report at 500 AVOW with two named reviewers | Contract |
| A code patch fixing a specific GitHub issue at 50 AVOW, code review by maintainer | Contract |

The rule of thumb: **if the work is fungible (many parallel earners, each contributing one of N), it's a Bounty. If the work is non-fungible (one specific deliverable, winner takes all), it's a Contract.**

## Why two primitives, not one with optional fields

We considered making `Bounty.board_schema_id` optional (single primitive, dual mode) and rejected it. Three reasons:

### 1. The dispute mechanisms are different

Bounty's dispute mechanism (per the bounty marketplace RFC) is a bond-then-board-operator-resolution flow. The board operator (`Schema::owner` of the bounty's board schema) resolves disputes. This works because Bounty is schema-anchored and the schema's owner is a natural arbiter.

Contract has no board. Disputes need a different mechanism: either the contract poster names an arbitrator at post time, or a federated arbitration pool resolves. The arbiter doesn't fit the "schema owner" model at all.

Forcing both into one primitive means the dispute handler has to branch on which mode the bounty is in. That's exactly the kind of muddy code-path optionality good chain primitives avoid.

### 2. The discovery shapes are different

Bounty discovery is "open bounties on schemas the worker has attested under" (the indexer matching service shipped in [ligate-api#75](https://github.com/ligate-io/ligate-api/pull/75)). Worker-to-bounty matching is schema-mediated.

Contract discovery is "open contracts in the worker's expertise area or by tag." Worker-to-contract matching is tag/skill-mediated. The indexer needs a different join shape.

Conflating them into one primitive means the indexer matching service grows two code paths and the query semantics get murky.

### 3. The reputation models are different

Bounty plus the credit-graph readout (item B of the [pre-testnet economics RFC](pre-testnet-economics.md)) builds per-schema linear-count reputation. An attestor accumulates a count per schema they've attested under. That count is the reputation surface.

Contract reputation is per-worker: completion rate, average rating, dispute outcomes. That's an Upwork-shape reputation, not a per-schema count.

If one primitive tried to serve both, the reputation queries would have to handle both shapes and the credit-graph readout would either drop contract activity (incomplete reputation) or include it confusingly (per-contract counts don't compose with per-schema counts).

## Contract primitive shape

New chain module at `crates/modules/contract/`, sibling to `crates/modules/bounty/`. Discriminant `CONTRACT_DISCRIMINANT = 23` (continues the Ligate-native sequence after `BOUNTY_DISCRIMINANT = 22`).

### State

```rust
pub struct Contract<S: Spec> {
    pub poster: S::Address,
    pub worker: Option<S::Address>,             // None until accepted
    pub arbiter: S::Address,                    // names a specific arbiter at post time
    pub criteria_doc_hash: Hash32,              // off-chain content-addressed criteria
    pub pool: Amount,                           // escrowed amount
    pub expiry_da_height: u64,
    pub dispute_window_blocks: u32,
    pub status: ContractStatus,
}

pub enum ContractStatus {
    Open,              // posted, accepting commits
    Committed,         // a worker has commit-revealed (or directly delivered)
    Delivered,         // worker submitted delivery; awaiting acceptance window
    Accepted,          // poster accepted; payout settled
    Rejected,          // poster rejected; in dispute
    Disputed,          // arbiter is resolving
    Cancelled,         // poster cancelled (only when Open + no commit)
    Expired,           // expiry_da_height passed before delivery
}
```

State maps:

```rust
pub struct Contracts<S: Spec> {
    #[id] id: ModuleId,
    #[module] bank: sov_bank::Bank<S>,
    #[module] attestation: attestation::AttestationModule<S>,

    pub contracts: StateMap<ContractId, Contract<S>>,
    pub commits: StateMap<(ContractId, S::Address), CommitRecord>,
    pub deliveries: StateMap<ContractId, DeliveryRecord<S>>,
    pub disputes: StateMap<ContractId, DisputeRecord<S>>,
    pub escrow: StateMap<ContractId, Amount>,
}
```

`ContractId` is `lct1...` Bech32m, derived from `(poster, criteria_doc_hash, nonce)`.

### CallMessages

```rust
pub enum CallMessage<S: Spec> {
    /// Post a new contract. Escrows `pool` AVOW. Names the arbiter at
    /// post time so all parties know the dispute path before committing.
    PostContract {
        arbiter: S::Address,
        criteria_doc_hash: Hash32,
        pool: Amount,
        expiry_da_height: u64,
        dispute_window_blocks: u32,
    },

    /// Worker signals intent to deliver. Posts a small commitment bond
    /// (refundable on accepted or expired-no-acceptance; forfeited on
    /// rejected-by-poster). v0 allows multiple workers to commit
    /// simultaneously; first to deliver an accepted submission wins
    /// the pool. v1 may add poster-side committed-worker selection.
    CommitToContract {
        contract_id: ContractId,
        commit_hash: Hash32,             // SHA-256 of the worker's deliverable
        bond: Amount,
    },

    /// Worker submits the deliverable. Reveals the preimage of the
    /// earlier commit_hash + provides an attestation_id (the worker's
    /// proof they've done the work, optionally signed by external
    /// reviewers).
    DeliverContract {
        contract_id: ContractId,
        deliverable_attestation_id: AttestationId,
    },

    /// Poster accepts the delivery. Pays pool to the worker (minus
    /// arbiter fee), returns worker's bond.
    AcceptDelivery {
        contract_id: ContractId,
    },

    /// Poster rejects the delivery. Bond is held until arbiter
    /// resolves. Transitions to Disputed.
    RejectDelivery {
        contract_id: ContractId,
        reason: DisputeGround,
    },

    /// Arbiter resolves a dispute. Pool + bond flow based on decision.
    ResolveContractDispute {
        contract_id: ContractId,
        decision: DisputeDecision,
    },

    /// Poster cancels an unaccepted contract. Refunds the escrow.
    /// Only callable when status is Open (no commits yet) or expired.
    CancelContract {
        contract_id: ContractId,
    },
}
```

### Why a commit-reveal flow

For non-fungible work-for-hire, commit-reveal protects workers from buyer-side stealing:

1. Worker submits a hash of their deliverable (the commit), bonding a small amount.
2. Poster reviews the commit (proof the worker is serious + locked in to a specific deliverable).
3. Worker reveals the deliverable via `DeliverContract` (or via off-chain delivery to the named arbiter).
4. Poster either accepts (pool flows to worker) or rejects (arbiter resolves).

The commit step prevents the buyer from seeing the deliverable, copying it, and refusing to sign. It also lets the worker prove timeline priority if multiple parties claim to have submitted the same work.

### Why a named arbiter at post time

Bounty disputes use the board operator (schema owner) as arbiter. Contract has no board. Two options:

1. **Federated arbitration pool** (like the prompt-evaluation pool from the [eval-pool RFC](prompt-evaluation-pool.md), but for arbitration instead of evaluation)
2. **Poster names arbiter at post time**

The RFC picks option 2 for v0. Naming at post time:

- Lets workers see the arbiter BEFORE committing (informed consent)
- Aligns with how Upwork / Bountysource / Aragon Court actually operate (named arbiters per contract or per platform)
- Doesn't require us to stand up an arbitration pool service before Contracts can work

Option 1 (federated pool) is the v1+ path: standardise an arbitration pool service, register it, posters reference it by id, the chain treats it as the arbiter. Same shape as the eval pool's evolution.

### Fee economics

Contract has a single arbiter fee (paid only if the arbiter is invoked):

- `pool * arbiter_fee_bps / 10000` flows to the arbiter on `ResolveContractDispute`
- Defaults: 500 bps (5%) at v0; configurable per-arbiter via off-chain agreement
- If no dispute, arbiter earns nothing (which is the right incentive: arbiters get paid for arbitration, not for being named)

No `fee_routing_bps` to a schema author, because there's no schema. This is the clean separation: schema authors earn from Bounties (where they add ongoing value); arbiters earn from Contracts (where they add ongoing value). Different primitives, different earning surfaces.

### Events

```rust
pub enum Event<S: Spec> {
    ContractPosted { contract_id, poster, arbiter, pool },
    WorkerCommitted { contract_id, worker, commit_hash, bond },
    ContractDelivered { contract_id, worker, deliverable_attestation_id },
    DeliveryAccepted { contract_id, worker, payout },
    DeliveryRejected { contract_id, worker, reason },
    ContractDisputeResolved { contract_id, decision, winner },
    ContractCancelled { contract_id, refunded_to_poster },
    ContractExpired { contract_id, refunded_to_poster },
}
```

### Composition with attestations

Contract's `deliverable_attestation_id` field points at the attestation module's `Attestation` record. The worker can:

- Submit the deliverable as a self-attestation under a personal schema, OR
- Submit the deliverable through a third-party witness (the eval pool, a credentialed reviewer, an arbitrator running as an attestor), OR
- Not attest at all (poster verifies off-chain, accepts directly)

This keeps Contract composable with the attestation primitive without requiring every contract to flow through an attestation. The chain enforces "there's an attestation_id you can point to if needed"; it doesn't enforce that the attestation is rich.

## Migration story

**Bounty stays as-is.** No changes to `crates/modules/bounty/`. Existing handler logic (#532) continues to serve fungible-evidence bounties. The bounty marketplace RFC and its sub-issues remain accurate for the Bounty primitive.

**Contract is additive.** New module, new discriminant, new state. Bumps `chain_hash` (just like Bounty did at #531) because the runtime composition changes. Wire format for Bounty is unchanged.

**v0.4.0 release bundles both.** The natural release point for this is the same release that bundles the bounty handlers + tests. By then we have:

- Bounty module shipped (handlers, tests, integration) ✅ done as of today
- Contract module shipped (skeleton + handlers + tests)
- One `chain_hash` bump from `0db23038…` to whatever v0.4.0 settles
- One devnet-3 cutover instead of cutting devnet-3 twice

Workers and buyers learn both primitives at the same time. The chain's narrative becomes "we support both fungible attestation marketplaces AND specific-deliverable contracts."

## Implementation sub-issues to file

1. **`crates/modules/contract/` skeleton.** Same shape as `crates/modules/bounty/` from #531: module struct + state maps + CallMessage + Event + no-op handler stubs. Ships as one `chain_hash`-bumping PR sibling to the bounty skeleton.
2. **Contract handler implementations.** Five handlers (post / commit / deliver / accept-or-reject / cancel) + arbiter resolution. Same shape as the bounty handler implementation (#532): real escrow flows + state writes + events.
3. **Contract integration tests.** Same shape as the bounty integration tests (#533): `TestRunner`-based, exercising each handler's happy and rejection paths.
4. **`docs/development/runbooks/contract-arbiter-onboarding.md`.** Operational runbook for arbiters: how to register, how to handle disputes, how the bond + payout flow works on chain.
5. **Indexer + API surface.** Same shape as the bounty matching endpoint ([ligate-api#75](https://github.com/ligate-io/ligate-api/pull/75)): a `GET /v1/contracts/open` endpoint for workers + a `GET /v1/contracts/{id}` detail endpoint. Plus indexer ingestion of `Contract/*` events.
6. **Mneme contract surfaces.** "Browse open contracts" + "deliver a contract" + "accept-or-dispute" UI flows. Lives in the Mneme wallet repo.

## Open questions for the next RFC pass

1. **Arbiter as attestor vs. as standalone party.** Should the arbiter be a registered AttestorSet (composes with reputation) or a plain `S::Address` (simpler, no composition with attestation reputation)? Lean plain address for v0; AttestorSet composition is a v1+ option.
2. **Multi-worker commits.** Should multiple workers be able to commit to the same contract in parallel, with the poster picking the winning one? v0 allows it; v1 may add explicit poster-side worker selection (e.g. `SelectWorker { contract_id, worker }` call). Need to think about how losing workers' bonds work.
3. **Tag / skill metadata.** Contracts need discovery beyond "browse all open." Off-chain tag system (indexer-side) vs on-chain `tags: SafeVec<SafeString, MAX>` field. Lean off-chain at v0; promote on-chain if discovery breaks at scale.
4. **Arbiter fee defaults.** 500 bps (5%) is plausible but not tested against real arbiter compensation expectations. Sanity-check against Aragon Court / Kleros / Upwork comparison.
5. **Cross-primitive composition.** A Bounty's acceptance predicate could in principle reference a Contract's outcome ("the bounty pays if a related contract was accepted"). v1+ composition; spec separately.

## Acceptance for this RFC

This document landing as `docs/protocol/contract-primitive.md` is the acceptance. The sub-issues filed alongside drive the actual implementation. The Bounty primitive that shipped today remains the canonical fungible-evidence marketplace; Contract adds the work-for-hire surface as a distinct first-class primitive.

The chain's economic story becomes: **schemas anchor reusable evidence shapes; bounties pay for fungible attestations within schemas; contracts pay for specific deliverables outside schemas.** Three primitives, three economic roles, each clean.
