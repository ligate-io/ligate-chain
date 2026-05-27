# Bounty marketplace (RFC v0.1)

Status: design draft. Tracks [#385](https://github.com/ligate-io/ligate-chain/issues/385). Implementation work, frontend product surface, and anchor-buyer outreach all branch off the spec landed here. Read alongside [`attestation-v0.md`](attestation-v0.md) for the attestation primitives this composes on top of.

## Why a bounty primitive

A bounty is the missing piece between a posted demand for evidence and the on-chain attestation primitive that supplies it. Without it, an AI lab that wants "100,000 attested original-text-by-a-human paragraphs" has no chain-side way to: (a) commit funds against the demand, (b) accept attestations against rules, (c) pay out attesters once acceptance conditions are met, (d) recover funds if the supply never materialises. Those four properties are the bounty.

The attestation module already supplies the evidence layer. The schema module supplies the matching contract (what counts as a valid attestation). The staking module ([#50](https://github.com/ligate-io/ligate-chain/issues/50)) supplies the bond layer needed for disputes. The bounty module sits across the three to thread the commercial path through.

This RFC specs the on-chain primitive. The off-chain marketplace UI (buyer dashboard, matching surface, status pages) and buyer-side go-to-market are tracked separately. The primitive needs to land first because everything else composes against it.

## What lives on chain vs off chain

The chain owns:

1. **Bounty objects**: `BountyId`, owner, schema-id, escrowed `Amount`, acceptance predicate, expiry, dispute window, status.
2. **Escrow + payout**: AVOW transferred into a bounty-owned escrow account at post, paid out at finalize, refunded on expiry-with-no-fulfillment.
3. **Acceptance tracking**: each `AttestationId` that the bounty owner claims against the bounty.
4. **Dispute primitives**: bond-posting, slashing, finalisation.

The chain does NOT own:

1. **Discovery / matching**: which open bounties is a given Mneme user eligible for? This is a Postgres-join question over (bounty schemas the user can satisfy) `JOIN` (open bounties on those schemas) `JOIN` (the user's existing reputation graph). Indexer territory, not consensus territory.
2. **UI**: buyer dashboard, posting wizard, payout history, dispute interface. Frontend.
3. **Quality scoring**: subjective "is this paragraph genuinely original?" judgement. The dispute mechanism is the on-chain check; quality scoring is off-chain (the buyer reviews and either accepts or disputes).

Discovery moving off-chain is the same boundary [`rest-api.md`](rest-api.md) draws for ledger queries. List/search/aggregate belongs in the indexer; the chain only does point lookups.

## Bounty as a schema

[#383](https://github.com/ligate-io/ligate-chain/issues/383) item 9 specifies bounty boards as schemas. This RFC adopts that framing exactly.

A bounty board is a `Schema` whose payload describes "the kind of evidence this board pays for." Concretely:

- Schema `themisra.bounty.v1` is the board-level schema, registered once per board.
- The board's `fee_routing_bps` + `fee_routing_addr` route the board operator's cut on every fulfilled bounty (the `Gitcoin Grants for evidence` revenue model from item 9).
- Individual bounties on the board are NOT schemas. They are bounty objects in the bounty module's state, parameterised by the board's schema id.

Example flow:

1. Anthropic registers a new schema `anthropic.original-text-corpus.v1` (the data contract: paragraph + author signature + claim that it is original).
2. Anthropic registers a bounty board schema `anthropic.original-text-bounty-board.v1` with `fee_routing_bps = 200` (2% to the board operator on each fulfilled bounty) and `fee_routing_addr = <anthropic-treasury-addr>`. Owner is Anthropic.
3. Anthropic (or any third party) posts a bounty against the board: "0.10 AVOW per attested-original paragraph, pool of 10,000 AVOW, expires 30 days, dispute window 7 days, must be attested by an attestor in `anthropic.curated-reviewers` attestor set."
4. Mneme users attest paragraphs against `anthropic.original-text-corpus.v1`. Each valid attestation is eligible for the bounty.
5. Anthropic (the bounty owner) submits a `ClaimBounty` tx pointing at each accepted `AttestationId`. The chain transfers 0.10 AVOW from escrow to the attester, minus the 2% board-operator routing fee.
6. After the dispute window passes without a `DisputeAttestation` from Anthropic, the payout is final.

Item 3 in #385 ("each bounty board is itself a schema with a routing fee back to the bounty operator") is exactly this shape.

## Module surface

### State

```rust
/// Bounty module state. Lives at the standard module-state path
/// (`/v1/modules/bounty/...`).
#[module]
pub struct Bounty<S: Spec> {
    /// All bounties, keyed by the deterministic `BountyId` derived
    /// from `(poster, board_schema_id, nonce)`. Status field tracks
    /// lifecycle; expired and finalised bounties stay in state for
    /// the audit trail (don't delete; pre-mainnet retention is
    /// cheap, post-mainnet we add an archive sink).
    #[state]
    pub bounties: StateMap<BountyId, BountyState<S>>,

    /// Reverse index: open bounty ids by board schema. Lets the
    /// matching service in the indexer fetch "what's open on this
    /// schema" in one chain call without scanning all bounties.
    /// `StateMap<SchemaId, SafeVec<BountyId>>` not a multimap because
    /// `SafeVec` bounds the list size, matching how the attestation
    /// module bounds `attestor_set.members`.
    #[state]
    pub open_by_schema: StateMap<SchemaId, SafeVec<BountyId>>,

    /// Per-bounty escrowed AVOW. Held as a virtual sub-balance under
    /// a deterministic module-controlled address; see "Escrow" below.
    #[state]
    pub escrow: StateMap<BountyId, Amount>,

    /// Active disputes, keyed by the `(BountyId, AttestationId)` pair
    /// that is contested. Stake-bond holders + resolution timestamps.
    #[state]
    pub disputes: StateMap<DisputeKey, DisputeState<S>>,
}
```

### Call messages

```rust
pub enum CallMessage<S: Spec> {
    /// Post a new bounty. Escrows `pool` AVOW from `sender` into the
    /// bounty's escrow account. Returns `BountyId` deterministically
    /// derived from `(sender, board_schema_id, nonce)` so off-chain
    /// previews can compute the id without a round-trip.
    PostBounty {
        board_schema_id: SchemaId,
        pool: Amount,
        per_attestation: Amount,
        acceptance: AcceptancePredicate,
        expiry_da_height: u64,
        dispute_window_blocks: u32,
    },

    /// Claim one or more attestations against the bounty. Each must
    /// satisfy `bounty.acceptance` and reference an existing valid
    /// `AttestationId`. Payout per attestation = `bounty.per_attestation`
    /// minus the board's `fee_routing_bps` share. Transfers happen
    /// at claim; disputes within the window can claw back.
    ClaimBounty {
        bounty_id: BountyId,
        claims: SafeVec<AttestationClaim>,
    },

    /// Dispute one specific claim within the dispute window. Bond
    /// equal to the claim's payout is required; bond is paid from
    /// `sender`'s balance. Resolution is a separate call.
    DisputeAttestation {
        bounty_id: BountyId,
        attestation_id: AttestationId,
        ground: DisputeGround,
    },

    /// Resolve an open dispute. Caller is the bounty board operator
    /// (the `Schema.owner` of `board_schema_id`). Decision is
    /// `accept` or `reject`; bond goes to the winner.
    ResolveDispute {
        bounty_id: BountyId,
        attestation_id: AttestationId,
        decision: DisputeDecision,
    },

    /// Cancel an unfunded or expired bounty. Refunds remaining
    /// escrow to the poster. Only callable by the poster, only when
    /// `now >= expiry_da_height` OR `escrow_remaining == pool` (no
    /// claims yet).
    CancelBounty {
        bounty_id: BountyId,
    },
}

/// Acceptance predicate. Inline rather than via a separate schema
/// field so the bounty itself encodes its rules (the board schema
/// describes the SHAPE of evidence; the bounty describes WHICH
/// evidence on that shape it pays for).
pub enum AcceptancePredicate {
    /// Any valid attestation against the board's schema counts.
    Any,
    /// Only attestations signed by attestors in this set count.
    AttestorSet(AttestorSetId),
    /// Only attestations where `payload_hash` matches one of these.
    /// Useful for "pay if you can prove you have this specific
    /// document/dataset" bounties.
    PayloadHashes(SafeVec<PayloadHash>),
    /// Composition: at least N peer attestations of the same
    /// payload hash by N distinct attestors. For "must be verified
    /// by 3 peers" patterns.
    PeerCount {
        min_attestors: u8,
    },
}
```

### Events

```rust
pub enum Event<S: Spec> {
    BountyPosted { bounty_id: BountyId, poster: S::Address, pool: Amount },
    BountyClaimed { bounty_id: BountyId, attestation_id: AttestationId, payout: Amount, attester: S::Address },
    BountyDisputed { bounty_id: BountyId, attestation_id: AttestationId, disputer: S::Address, bond: Amount },
    DisputeResolved { bounty_id: BountyId, attestation_id: AttestationId, decision: DisputeDecision, bond_recipient: S::Address },
    BountyExpired { bounty_id: BountyId, refunded_to_poster: Amount },
}
```

These map to indexer subscriptions (per [#92](https://github.com/ligate-io/ligate-chain/issues/92) when WebSocket lands) so the matching service knows to surface new bounties to Mneme users within seconds.

## Escrow

Two implementation options. RFC picks option B for v0.

### A. Real bank-module sub-accounts

Each bounty gets a deterministic `S::Address` (derived from `BountyId`), funds get transferred there at `PostBounty`, paid out via real `bank` transfers at `ClaimBounty`.

Pros: re-uses the bank's well-tested transfer path, no new accounting primitives, balances visible in the standard `bank.GetBalance` query.

Cons: every bounty consumes an address-derivation pass, and the address has no associated private key (bounty-controlled, not user-controlled), which is unusual at the bank-module level.

### B. Module-internal `escrow` StateMap

Track escrowed amounts in the bounty module's own state, separate from the bank's view. The bank holds total module-locked AVOW under one synthetic module address; the bounty module's `escrow` map says how that total breaks down per bounty.

Pros: explicit accounting boundary (the chain can audit `sum(escrow values) == bank.balance(bounty_module_addr)` as a state invariant per slot). Avoids unowned bank-level addresses.

Cons: payout requires a bank `Transfer` initiated by the bounty module's authority, which means the bounty module needs a transfer-authority capability the bank module's call dispatcher honours. SDK supports this via the `module_authority` pattern; we use it already in the attestation module's fee-routing path.

**Decision: B.** The auditability story (sum-invariant) is the bigger win, and the transfer-authority plumbing already exists. Add the invariant as a runtime check that fires every slot in `debug` builds and on every Nth slot in `release`.

## Matching

Off-chain in the indexer ([#91](https://github.com/ligate-io/ligate-chain/issues/91)). Concrete flow:

1. User submits an attestation in Mneme (e.g. `themisra.original-text.v1` paragraph).
2. The attestation lands on chain, indexer ingests the event.
3. Indexer joins (this attestation's schema) `JOIN` (open bounties on that schema) `JOIN` (the bounty's acceptance predicate matches this attestor / payload hash / peer count).
4. Indexer pushes the result to Mneme via the existing chain-event firehose ([#92](https://github.com/ligate-io/ligate-chain/issues/92), pending) or a direct REST query.
5. Mneme renders "+0.05 AVOW from `<bounty board>` for this attestation" inline with the post-attest confirmation.

The chain itself never does matching. It just publishes events and answers point lookups. This is the same boundary the attestation module already respects.

## Dispute mechanism

Three options were considered (per #385 open decision 3). RFC picks the hybrid path.

| Mechanism | How it works | Trade-off |
|---|---|---|
| Pure stake-bond | Disputer posts bond = payout amount. Bounty operator resolves; bond goes to winner. | Cleanest. Trusts the bounty operator to resolve fairly. Acceptable when the operator IS the buyer (Anthropic resolves disputes on Anthropic's own bounty). Breaks if the operator-and-buyer model diverges (Gitcoin-style operator running someone else's bounty). |
| Pure social | Off-chain arbitration body (Kleros-style or schema-governed). | Slow, expensive, hard to onboard. Defers the on-chain ship. |
| Hybrid (chosen) | Stake-bond by default. If the disputer wants to escalate above the operator's decision, they can re-bond at the schema-curator level (the `Schema.owner` of the board schema). v2 adds slashing-aware staking from #50 + #51 for serial bad-actor patterns. | Phased path: v0 ships the bond layer, v1 adds curator-level escalation, v2 adds the full disputes module. |

The v0 ship below specs only the bond-then-operator-resolution layer. Curator escalation + slashing land in follow-ups against [#50](https://github.com/ligate-io/ligate-chain/issues/50) and [#51](https://github.com/ligate-io/ligate-chain/issues/51).

## Open decisions (resolved)

From #385's open-decisions list, with recommendations:

1. **First-party Ligate Labs marketplace vs permissionless from day one.** Recommendation: **permissionless from day one**, with Ligate Labs running a first-party board (`ligate.canonical-bounty-board.v1`) as the reference implementation + the place anchor partners post initially. The protocol itself shouldn't bake in a privileged operator; the canonical board is just a well-known schema id. This avoids the [#383](https://github.com/ligate-io/ligate-chain/issues/383) trap of "we gatekeep the marketplace and become the bottleneck."
2. **Routing fee on bounties (1 to 3% per #383).** Recommendation: **default 2% (200 bps)** on the Ligate-Labs first-party board, configurable by board owner from 0 to 1000 bps (10%) on third-party boards. Hardcoded ceiling at 1000 bps in the schema validation so operators can't grief the buyers they're supposedly serving. Above-ceiling needs a governance vote post-launch.
3. **Dispute resolution mechanism.** Recommendation: **hybrid** (bond by default, curator escalation v1, full slashing v2). See the table above.
4. **Anchor buyers (3 to 5).** Recommendation: **separate work item, gated on the spec above shipping.** The outreach conversation is "here's the on-chain primitive, here's how your bounty would look in 30 lines of TypeScript, here's the LOI ask." That ask needs the spec to be concrete. Once this RFC lands, file a sub-issue to track the LOI conversations in the research repo's outreach surface (parallel to the existing design-partners flow).

## Sub-issues to file after this RFC merges

1. **`bounty` module crate skeleton**: `crates/modules/bounty/{lib.rs,query.rs,metrics.rs,Cargo.toml}` + module composition into `Runtime`. State + CallMessage + Event enums + no-op handlers. Tests stubbed. ~2-3 days of focused Rust work.
2. **Anchor-buyer LOI tracking**: in `~/Desktop/ligate-docs/research/outreach/bounty-buyers/` parallel to the existing design-partners flow. Identify 3 to 5 candidates (one AI lab, one dataset company, one compliance buyer at minimum), draft cover-email + scope template.
3. **Mneme bounty UI**: extend the Mneme wallet ([#55](https://github.com/ligate-io/ligate-chain/issues/55)) with a "post-attest" panel that calls the indexer for matching bounties and surfaces "+X AVOW from `<board>`" inline.
4. **Indexer bounty matching service**: extends [#91](https://github.com/ligate-io/ligate-chain/issues/91) with the per-attestation join described in "Matching" above.
5. **Bounty board operator dashboard**: web UI for posting a bounty, monitoring inflow, claiming, disputing. v0 can be CLI; v1 a web app. Out of scope for the chain itself.

## Out of scope for this RFC

- The chain-side staking module ([#50](https://github.com/ligate-io/ligate-chain/issues/50)) that the v1 dispute mechanism depends on.
- The disputes module ([#51](https://github.com/ligate-io/ligate-chain/issues/51)) that the v2 slashing path depends on.
- Cross-chain bridges. A bounty paid in AVOW could in principle be paid in bridged USDC at some point; out of scope here, blocked on [#73](https://github.com/ligate-io/ligate-chain/issues/73).
- Refundable attestations ([#383](https://github.com/ligate-io/ligate-chain/issues/383) item 10). Composable with bounties later; not a v0 dependency.

## Open questions for the next RFC pass

These got worked through but I want a second pair of eyes before they're locked.

1. **AcceptancePredicate composition**: should `AcceptancePredicate` be unioned (any of) or intersected (all of)? Above spec implies "exactly one", which is the simplest. Compound predicates can be added in v1 as `AcceptancePredicate::All(SafeVec<AcceptancePredicate>)` without breaking the v0 wire format.
2. **Bounty pool top-ups**: should `PostBounty` be the only entry point, or should there be an `IncreaseBountyPool` call so a buyer can add funds to an existing bounty without changing its terms? Easy to add; question is whether the lifecycle simplifies (one PostBounty per logical bounty) or is worth complicating for the UX win.
3. **Expiry granularity**: `expiry_da_height` is in DA blocks (current celestia.toml's 6s block time means ~14400 blocks/day). Is that the right unit, or do we want `expiry_unix_seconds`? Block height is the canonical chain time; unix time needs an oracle. Sticking with DA height.
4. **Cancel-with-pending-claims behaviour**: can a poster cancel a bounty that has open dispute-window claims? Current spec says only when expired OR no claims. Should it instead drain claims-in-flight first? Lean "drain first" but want to confirm.

## Acceptance for this RFC

This document landing as `docs/protocol/bounty-marketplace.md` is the acceptance. Each sub-issue under "Sub-issues to file" closes #385's product surface piece-by-piece. #385 itself stays open until at least one real bounty is posted on the live chain by a non-Ligate-Labs buyer.
