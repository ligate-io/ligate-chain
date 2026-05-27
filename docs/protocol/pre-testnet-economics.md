# Pre-testnet economics (RFC v0.1)

Status: design draft. Tracks [#383](https://github.com/ligate-io/ligate-chain/issues/383). The long-form Token Model RFC is [#258](https://github.com/ligate-io/ligate-chain/issues/258); this doc is the pre-testnet cut of that work: which primitives ship, which are spec-only, which defer.

This RFC composes on top of the bounty marketplace RFC at [`bounty-marketplace.md`](bounty-marketplace.md) (chain#518). The bounty primitive is the centrepiece of the labor-vector story below; the rest of this doc covers everything else.

## The shape of the question

[#383](https://github.com/ligate-io/ligate-chain/issues/383) catalogues 14 primitives across three earning vectors (operator, capital, labor) plus 11 open decisions. We can't ship all 14 by testnet, and we can't spec all 14 either: at some point the design effort itself becomes the bottleneck. This RFC picks the cut.

Three guiding rules from #383's trap analysis carry over unchanged:

1. **Not GameFi.** Every reward loop is buyer-funded, never inflation-funded.
2. **Not Mechanical Turk.** Reputation accumulation is the moat over data-labeling marketplaces.
3. **Not data-union graveyard.** The "earn from your data" pitch needs at least one concrete buyer LOI before testnet (tracked in [chain#520](https://github.com/ligate-io/ligate-chain/issues/520)).

## Three earning vectors, restated

| Vector | Audience | Source of money | Pre-testnet bet |
|--------|----------|-----------------|-----------------|
| **Operator** | Sysadmins, infra companies | Builder share + priority fees + protocol take | Ligate Labs runs everything; permissionless paths specced for v1 |
| **Capital** | $LGT holders, retail, funds | Yield share from attestation fees via staking | None at testnet (depends on staking #50); narrative is "yield comes online with staking" |
| **Labor** | Mneme users, evidence producers | Buyer-funded bounties | Bounty primitive ships per [#385 RFC](bounty-marketplace.md); core testnet pitch |

The labor vector is the headline because it's the only one we can fully ship by testnet with a concrete buyer relationship. The operator and capital stories ship as roadmap with credible specs.

## Decisions, resolved

The 11 open decisions in [#383](https://github.com/ligate-io/ligate-chain/issues/383) §"Decisions needed before specs", with recommended answers:

| # | Question | Recommendation | Rationale |
|---|----------|----------------|-----------|
| 1 | Open attestor sets in v1 or v1+? | **v1+** | Depends on [#50](https://github.com/ligate-io/ligate-chain/issues/50) staking; multi-month. Testnet ships federated-only. |
| 2 | Bounty attestations: first-party or permissionless? | **Permissionless, with first-party reference board** | Already resolved in [#385 RFC](bounty-marketplace.md). |
| 3 | Insurance pool: protocol-owned or third-party? | **Defer to v1+** | Not testnet-critical. Real demand surfaces post-mainnet when the consumer side of high-stakes attestations (medical, legal) shows up. |
| 4 | Cross-chain bridge: native or third-party? | **Defer to post-mainnet**; when ready, use Hyperlane ([#73](https://github.com/ligate-io/ligate-chain/issues/73)). | Native bridges are mainnet-era infra. The credit-graph monetization story (item B) works fine via Ligate-side queries for the first year. |
| 5 | Schema marketplace fee (protocol take): 0/5/10/15%? | **5% mainnet, 0% testnet** | Lower than App Store's 15% because we're earlier and want developer adoption; high enough to fund protocol dev. Testnet runs at 0% so developers don't model the take rate into their margins until mainnet locks the value. |
| 6 | Refundable attestations: default or opt-in? | **Defer to v1+** | UX nice-to-have. Bond/refund pool requires the staking module too. |
| 7 | Reputation portability kernel: who designs? | **LIP candidate, defer to v1** | Cross-schema kernel design is research-paper-shaped, not a single-PR shape. Worth spending in v1 design cycle. |
| 8 | Reputation kernel shape: linear / recency / attestor-quality / hybrid? | **Linear count for v0 → recency-weighted at v1 → attestor-quality at v2** | Compose forward. Linear count is computationally trivial and clients can layer their own weighting on top until we standardise. |
| 9 | Pre-testnet minimum viable set: which of items 1-14? | See ship/spec/defer matrix below. | |
| 10 | Pre-testnet minimum per vector? | Labor: ship bounty primitive. Operator: ship federated-only with permissionless path specced. Capital: ship the staking design only, no actual staking module yet. | |
| 11 | Public RPC/indexer incentive at testnet: Ligate Labs only or with third-party operators? | **Ligate Labs only at testnet** | RPC marketplace ([#13](https://github.com/ligate-io/ligate-chain/issues/13) in #383) is multi-month operator-onboarding work. Don't gate testnet on it. |

## Ship / spec / defer matrix

Across the 14 primitives plus the three "always there" items (B reputation graph, C fee-routing developer pitch, A stake-to-attest delegation):

| Ref | Primitive | Pre-testnet | Why |
|---|-----------|---|-----|
| A | Stake-to-attest delegation | **Spec** | Depends on [#50](https://github.com/ligate-io/ligate-chain/issues/50) staking module. Ship the design as part of [#258](https://github.com/ligate-io/ligate-chain/issues/258); shippable code is post-testnet. |
| B | Attestation credit graph | **Ship (linear count v0)** | Already on chain as event-level data. v0 kernel = "count of attestations against schema X by address Y" rendered by the indexer. No new chain code needed; this is a query-side feature. |
| C | Fee-routing developer pitch | **Ship (marketing flip)** | The mechanism (`Schema.fee_routing_bps`) is already on chain. What's missing is the dev-facing story. Update README + docs + a dev-facing blog post that uses fee routing as the headline. |
| 1 | Open attestor sets | **Defer** | Blocked on #50 staking. Permissionless attestor entry is v1+. |
| 2 | Bounty attestations | **Ship** | Bounty primitive specced in [`bounty-marketplace.md`](bounty-marketplace.md). Module crate skeleton tracked in [#519](https://github.com/ligate-io/ligate-chain/issues/519). |
| 3 | Witness / oracle attestations | **Ship (no new chain code)** | An "oracle attestation" is just an attestation against a schema where the attestor pool is curated for off-chain witnessing. Mechanism exists; pitch + onboarding flow is marketing/docs work, not chain. |
| 4 | Reputation-as-collateral lending | **Defer to v2+** | Meta-attestation flow. Depends on lending product surface that doesn't exist. |
| 5 | Self-attestation with downstream value | **Ship (no new chain code)** | Same mechanism as bounty / witness attestations from chain's POV. Value capture is downstream consumers' responsibility. |
| 6 | Insurance pool | **Defer to v1+** | Real demand surfaces post-mainnet. |
| 7 | Schema marketplace fee (protocol take) | **Spec only** | Decision landed (5% mainnet, 0% testnet). Implementation = adding a treasury-route step in the fee distribution path; one-PR change. Spec now, ship at mainnet. |
| 8 | Cross-chain bridge | **Defer to post-mainnet** | EVM-side adoption is the bet; not testnet-blocking. |
| 9 | Bounty marketplaces as schemas | **Ship** | Specced in [`bounty-marketplace.md`](bounty-marketplace.md). |
| 10 | Refundable attestations | **Defer to v1+** | Friction reduction; not essential. |
| 11 | Reputation portability across schemas | **Spec only (LIP candidate)** | Cross-schema kernel is its own design pass. Spec lives in #258 long-form RFC; no v0 ship. |
| 12 | Verified-by-stake schema launch | **Defer to v1+** | Depends on #50 staking + #51 disputes. |
| 13 | Paid RPC / indexer marketplace | **Defer to v1+** | Operator onboarding is its own multi-month workstream. |
| 14 | Bridge relayer incentive | **Defer (paired with #8)** | |

**Shipping count: 5 primitives (B + C + 2 + 3 + 5 + 9; some are zero-code marketing flips).** Specs without code: A + 7 + 11 (3 primitives). Deferred: 1 + 4 + 6 + 8 + 10 + 12 + 13 + 14 (8 primitives, all v1+ or post-mainnet).

The cut is honest: more than half of #383's surface is post-mainnet because most of it depends on the staking module (#50) that hasn't started. What ships before testnet is the labor vector (bounty primitive) and the credit-graph readout that makes it accumulate.

## Minimum viable set per vector

### Labor (the testnet headline)

- ✅ Bounty primitive on chain ([`bounty-marketplace.md`](bounty-marketplace.md), [#519](https://github.com/ligate-io/ligate-chain/issues/519) module crate)
- ✅ Indexer matching service ([ligate-api#73](https://github.com/ligate-io/ligate-api/issues/73))
- ✅ Mneme post-attest "+X AVOW from `<board>`" panel ([ligate-mneme#1](https://github.com/ligate-io/ligate-mneme/issues/1))
- ✅ One anchor-buyer LOI ([#520](https://github.com/ligate-io/ligate-chain/issues/520))
- ✅ Credit-graph readout in the indexer (linear count v0, no new chain code)
- ❌ Open attestor sets, refundable attestations, insurance pool, all v1+

The bounty primitive shipping with a real buyer LOI is the testnet pitch. Everything else in the labor vector is "future yield from this same buyer relationship as it grows."

### Operator (federated → permissionless roadmap)

- ✅ Federated sequencer (Ligate Labs only at testnet)
- ✅ Schema-curated attestor federations
- ✅ Fee-routing developer pitch (marketing flip on existing primitive)
- 🟡 Sequencer leader rotation ([#82](https://github.com/ligate-io/ligate-chain/issues/82)), specced for v1, shipped post-testnet
- 🟡 Permissionless attestor entry ([#50](https://github.com/ligate-io/ligate-chain/issues/50) dependency), v1+
- ❌ Paid RPC / indexer marketplace (#13), v1+ once external operators want to onboard
- ❌ Bridge relayer, paired with the bridge, post-mainnet

The testnet operator story is "Ligate Labs runs the rails today; here's the v1 / v1+ path for taking over the role."

### Capital (the $LGT yield narrative)

- ❌ Stake-to-attest delegation (item A), specced, not shipped at testnet
- ❌ Insurance pool stakers, v1+
- ❌ Refundable attestation bond pool stakers, v1+
- ❌ Verified-by-stake schema bonding, v1+

The capital vector ships at v1, not at testnet. At testnet, the $LGT story is "this is the token bounty buyers pay in, attestors receive, and operators eventually earn from. Yield surfaces with the staking module." Holders without operator infrastructure don't have a yield surface yet; this is the post-testnet gap that #50 closes.

This is the part where we have to be honest with the market: testnet is the labor + bounty story, not the staking-yield story.

## What this RFC does NOT decide

Carries over to #258 long-form:

- The exact `fee_routing_bps` default in CLI tooling (a CLI ergonomics question; tracked separately).
- The full reputation kernel shape (linear v0, beyond is research-shaped).
- Cross-schema reputation transfer math (LIP candidate).
- Inflation policy. We've ruled OUT inflation for reward loops here; whether there's ANY inflation (e.g. attestor-incentive subsidy in the first year, sunsetting on schedule) is its own decision in #258.
- Token supply schedule, treasury vesting, foundation allocation, etc. Tokenomics-paper territory, not chain-runtime.

## Sub-issues to file after this RFC merges

1. **Update README + landing.ligate.io with the fee-routing developer pitch** (item C). Marketing flip on an existing primitive. Doesn't touch chain code. Tracked separately so a designer can own the copy.
2. **Indexer: linear-count reputation readout** (item B v0). Adds `GET /v1/stats/reputation/{address}?schema={schema_id}` returning the count. Lives in ligate-api; ~half-day of work.
3. **Schema marketplace fee mechanism** (item 7). Adds a `protocol_take_bps` parameter to the schema-fee distribution path. Spec lives here; implementation is one PR in `crates/modules/attestation/` to add the treasury split. Ship at mainnet, not testnet.
4. **Pre-testnet capital-vector narrative + roadmap doc** (the "yield comes online with staking" story). Lives in `docs/protocol/staking-roadmap.md` and points at #50. Communicates the v0 → v1 path to holders without overpromising.

(Sub-issue 5+: the existing chain#519, #520, #521 + ligate-api#73 + ligate-mneme#1 from the bounty RFC already cover the labor-vector ship list.)

## Acceptance for this RFC

This document landing as `docs/protocol/pre-testnet-economics.md` is the acceptance. #383 stays open until the testnet launch post can quote:

- At least one real buyer LOI for the bounty primitive (#520 dependency)
- A live `GET /v1/stats/reputation/{address}` endpoint (sub-issue 2 above)
- README + landing page leading with the fee-routing developer pitch (sub-issue 1 above)
- A coherent "yield narrative" doc that points at the v1 staking timeline (sub-issue 4 above)

Until then, the testnet launch defends a thin economic story even if the chain itself is technically sound. The RFC is the plan for not letting that happen.
