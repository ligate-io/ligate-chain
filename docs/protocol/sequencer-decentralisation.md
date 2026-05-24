# Sequencer Decentralisation, Design Decision

**Status:** decision recorded for v1 implementation; revisit triggers listed in §7
**Scope:** how Ligate Chain moves off the v0 single-sequencer model toward mainnet
**Applies to:** sequencer plumbing in `crates/rollup` + `sov-sequencer` integration; the staking module (#50) and disputes module (#51) inherit constraints from this doc
**Tracks:** [ligate-io/ligate-chain#82](https://github.com/ligate-io/ligate-chain/issues/82)

---

## TL;DR

**v1 step 1: internal leader rotation.** Ligate Labs runs N=3 sequencer instances; leader rotates per slot via the Sovereign SDK's existing pattern. Ships in weeks. Kills the single-process SPOF. Doesn't lock us into anything.

**v1 step 2 / pre-mainnet: federated validator set.** 5-10 external orgs, stake-weighted leader rotation. Pairs naturally with the staking module (#50) and disputes module (#51). This is the actual PoUA substrate.

**Out of scope:** shared sequencer (Astria / Espresso / Skip) — contradicts the sovereign-rollup positioning the chain and the PoUA paper both depend on. Based rollup (Celestia validators sequence) — re-evaluate as v2/v3 future-work once Sovereign SDK patterns mature.

---

## 1. Context

`ligate-devnet-1` runs a single sequencer operated by Ligate Labs. The chain's **safety** doesn't depend on this: every full node re-derives the state root by replaying the canonical batch stream off Celestia. A misbehaving sequencer can stall blocks but cannot fabricate state.

What the single-sequencer model **does** depend on for:

- **Liveness.** If our sequencer process dies, the chain stops proposing blocks until we restart it. Today this is one VM, one process, one operator.
- **Censorship-resistance.** If we (Ligate Labs) decide to drop a specific transaction, there is no protocol-level recourse. Force-include path (#81) is the eventual safety valve, but it's not implemented yet.
- **Credible neutrality.** The narrative we sell to design partners, regulators, and downstream developers is "permissionless attestation protocol." Running every block ourselves is in tension with that narrative regardless of how trustworthy we behave.

This doc decides how the chain moves off the single-sequencer model, with enough specificity that the next person implementing it (or the next investor reading our roadmap) does not need to re-litigate the call.

---

## 2. Requirements

Whatever option we pick must satisfy:

**R1. PoUA-compatible.** We control the validator-weight function. Outsourcing consensus to a third-party sequencing network means the PoUA mechanism we've spent v0.7 papering on validator-side reputation has nothing to attach to. This is non-negotiable: the chain's economic moat is PoUA; PoUA needs a validator set we own.

**R2. Latency budget intact.** Current target: sub-second proposal latency, ~10s slot cadence on devnet (matches Celestia mocha-4 block time). Any option that adds a remote-consensus hop to the critical path needs an explicit answer for how that hop fits.

**R3. Censorship-resistance via two layers.** Multi-party sequencing + force-include path (#81). One alone is insufficient; together they cover honest-failure and adversarial-leader cases.

**R4. Sovereign SDK upstream-compatible.** The chain is a fork pin on `Sovereign-Labs/sovereign-sdk`. Whatever sequencer pattern we pick should track an upstream-maintained code path so we don't drift into permanent-fork territory.

**R5. Operationally affordable at v1 scale.** v1 is "post-devnet, pre-mainnet." Budget is small. Anything that requires >10 external operators or specialty hardware is post-v1.

---

## 3. Options

### 3.1 Internal leader rotation

**Mechanism.** Ligate Labs runs N=3 sequencer instances on independent VMs. The Sovereign SDK's preferred-sequencer pattern is extended with a deterministic per-slot leader assignment; on failure-detect the next-in-rotation takes over.

**Latency.** No external hop. Same as today.

**Security model.** Liveness improves: any 1-of-3 alive keeps the chain proposing. Safety unchanged (full nodes re-verify). Censorship-resistance unchanged from today (still all Ligate Labs); the force-include path (#81) remains the only adversarial-recourse mechanism.

**PoUA compatibility.** Fully compatible. Validator-weight function is trivial (all weights = 1/3) but the slot-level book-keeping that PoUA needs (proposer, votes) exists. Same plumbing extends to the federated case in 3.2.

**Ops cost.** Three VMs instead of one. Cheap. Already-built monitoring (Caddyfile, Grafana dashboards, dead-man's switch) extends with per-instance labels.

**Partner story.** "We removed the single-process SPOF; multi-org follows in step 2." Honest and easy.

**Effort.** Weeks. Most of the code path exists upstream.

### 3.2 Federated validator set

**Mechanism.** 5-10 external organisations operate sequencer/validator nodes. Each bonds `$AVOW` via the staking module (#50). Leader rotation is stake-weighted. Slashing (#51) penalises equivocation, downtime, and the slashing conditions specified in the PoUA paper (§4.5).

**Latency.** One additional hop per slot for vote aggregation (the BFT round). At 5-10 validators in good network conditions this is sub-100ms; well within the 10s slot budget.

**Security model.** BFT safety under `f < n/3` Byzantine. Liveness under `f < n/3` failed. Censorship-resistance: any single validator can be censored by 2/3 of others, but a 1/3+ Byzantine set could censor specific transactions until force-include (#81) kicks in. The two layers together cover the case.

**PoUA compatibility.** This IS the substrate the PoUA paper assumes. §4 specification, §7 implementation, §A.4 detection procedures all land here.

**Ops cost.** Significant. Partner recruitment (5-10 orgs willing to operate a validator, hold stake, take operational responsibility). Coordination overhead (chain upgrades, parameter changes, incident response). Slashing logic + dispute resolution path that has to actually work the first time.

**Partner story.** This is the story that closes design partners and unblocks the credible-neutrality claim.

**Effort.** Months. Plus partner-recruitment work that runs in parallel and gates the launch.

### 3.3 Shared sequencer (Astria / Espresso / Skip)

**Mechanism.** A separate, cross-rollup sequencing network orders our transactions. We submit a stream, they output a canonical ordering, our nodes execute against that ordering.

**Latency.** Depends on the shared network's consensus. Astria targets sub-second; Espresso targets a few hundred ms. Likely fine for our 10s slot budget but introduces a hop we don't control.

**Security model.** We inherit the shared network's safety + liveness. Adds them as a critical dependency.

**PoUA compatibility.** **Fails R1.** PoUA needs our own validator set with our own weight function. A shared sequencer means *they* sequence; the PoUA mechanism design has no validator to attach to. We could maybe layer PoUA reputation on top of *our execution layer* validators, but that's a different and weaker security argument: the paper's cost-to-attack premium κ requires the reputation-weighted set to also be the consensus set.

**Ops cost.** Low integration cost. High narrative cost (now we have to explain why we're a "sovereign rollup" when we don't sequence our own blocks).

**Partner story.** Cheaper to ship, but undermines the whole "we own the chain" frame that Mneme, Themisra-Pro, and Iris-Enterprise all sell from.

**Effort.** Days for integration, weeks for proper testing. Cheap if we picked this; we're not picking this.

### 3.4 Based rollup (Celestia validators sequence)

**Mechanism.** Celestia validators themselves sequence Ligate transactions. Submitters publish into Celestia blob space; the ordering is the Celestia block ordering; no separate Ligate sequencer exists.

**Latency.** Bound to Celestia block time. Mocha-4 today: ~12s. Mainnet Celestia: ~12s with intent of dropping toward 6s. That's our minimum slot cadence and we have no lever to improve it.

**Security model.** Aligns the DA layer and the sequencing layer (no separate trust assumption). Strong.

**PoUA compatibility.** Same issue as 3.3 — we don't own the consensus, the PoUA mechanism has nothing to attach to.

**Ops cost.** Cutting-edge; the Sovereign SDK based-rollup pattern is not production-mature. We'd be doing R&D, not integration.

**Partner story.** Strongest "decentralised by construction" story of the four. Weakest near-term shippability.

**Effort.** Cutting-edge. Multi-quarter. Track but do not commit.

---

## 4. Decision

**v1 step 1 (next ~6-8 weeks): Option 3.1 — internal leader rotation.**

- N=3 sequencer instances, all operated by Ligate Labs, deterministic per-slot leader assignment.
- Kills the single-process SPOF.
- Pure-ops decentralisation, not protocol decentralisation. Acknowledged honestly in partner conversations as "step 1 of the path; step 2 (federated set) is in active design."
- Buys us months of runway to do the harder step 2 work without holding mainnet hostage.

**v1 step 2 / pre-mainnet: Option 3.2 — federated validator set.**

- 5-10 external organisations (target: 7).
- Stake-weighted leader rotation via the staking module (#50).
- Slashing via the disputes module (#51) covers equivocation + the PoUA paper's A1 condition (invalid attestation inclusion).
- Pairs with PoUA layer-1 / layer-2 / layer-3 defences (#129-#134) when those land on the v3 path.
- Partner recruitment runs in parallel; chain-side work assumes 7 operators by default with parameters tunable for 5-10.

**Explicitly not picked:**

- **Option 3.3 (shared sequencer).** Fails R1. Outsourcing consensus contradicts the PoUA mechanism design we've spent v0.7 writing. Bring back to the table only if PoUA itself gets de-scoped, which we are not planning to do.
- **Option 3.4 (based rollup).** Latency-bound to Celestia block time with no lever. Sovereign SDK pattern not mature. Re-evaluate as v2/v3 future-work; meanwhile track Sovereign Labs' upstream work on the pattern.

---

## 5. Implementation plan for Option 3.1

### Audit findings (chain#410)

The plumbing audit located the seam in two places:

- **Config seam (ours):** `devnet/rollup.toml`'s `[sequencer.preferred]` section is missing an optional `postgres_config` block. When that block is present, the SDK switches from single-instance RocksDb to Postgres-backed multi-instance with leader election. Today the block is absent — that is why we run single-sequencer.
- **Code seam (upstream SDK):** `sov-sequencer/src/preferred/db/mod.rs:543-564` is the actual `match ConfiguredNodeRole {...}` branch where leader-vs-replica behaviour diverges. `// SEAM:` comment in `crates/rollup/src/lib.rs` (next to `NodeRole`) marks the discovery point for readers.

The Sovereign SDK **already implements** the bulk of Option 3.1:

- `ConfiguredNodeRole::{Leader, Replica, ReplicaNoLeaderSync, DbElected}` enum at `crates/full-node/full-node-configs/src/sequencer.rs:139`
- Postgres heartbeat task at `preferred/db/heartbeat_task.rs`
- Replica sync task at `preferred/replica/replica_sync_task.rs`
- `LeaderElectionConfig` (heartbeat interval, leader timeout, grace period) with sensible defaults
- SQL `is_leader()` function in the Postgres migrations
- Tests at `preferred/db/postgres/tests/leader_election.rs`

`DbElected` mode is exactly what we want for Option 3.1: each instance heartbeats into Postgres, one wins the election, others run as replicas synced from the leader via Postgres state sync. On leader failure (heartbeat timeout + grace period elapsed) one of the replicas takes over.

### Revised sub-issue list

Original §5 from v1 of this doc assumed we'd implement `LeaderSchedule` + view-change from scratch. The audit makes most of that disappear; what's left is operational:

1. ✅ **Plumbing audit.** Done. `// SEAM:` comment in `crates/rollup/src/lib.rs`, this section updated. Filed as chain#410.

2. ~~Deterministic per-slot leader (`LeaderSchedule`)~~ — **dropped.** SDK's `DbElected` Postgres election replaces this. Heartbeat-based instead of slot-based, which is arguably better (no two-sequencers-both-think-they're-leader race at slot boundaries).

3. ~~Failure detection + view-change~~ — **dropped.** SDK's heartbeat timeout + grace period IS the failure detection; the next election cycle IS the view-change. Both fully configurable via `LeaderElectionConfig`.

4. **Postgres provisioning + config.** Provision a managed Postgres instance (Cloud SQL with regional HA, or a small managed Postgres on the same VPC) accessible from every sequencer VM. Wire `[sequencer.preferred.postgres]` into `devnet/rollup.toml` with `node_role = "DbElected"` for all three sequencer instances, distinct `node_id` per instance. Acceptance: three local sequencers running against a single Postgres see one elected leader and two replicas; killing the leader triggers a new election within the configured timeout.

5. **Multi-sequencer prod deploy.** Spin up two additional GCP instances (`ligate-node-2`, `ligate-node-3`); extend the cloud-init template for per-instance index. Update Caddyfile to fan reads out across all healthy instances and route writes (`POST /v1/sequencer/txs`) to the current leader (Postgres-derived via a small health endpoint, or the SDK's existing leader-info API if it exposes one). Add per-instance Grafana labels. Acceptance: prod runs 3 sequencers for 24h with at least one forced-failover incident successfully handled.

6. **Force-include observability stub.** Wire `ligate_forced_inclusion_total{reason}` as a 0-emitting counter for the eventual #81 work. Acceptance: counter visible in `/metrics`.

7. **Documentation.** Update `docs/development/public-devnet-deploy.md` for the multi-sequencer topology; add `docs/development/runbooks/multi-sequencer-failover.md`. Cover the Postgres dependency (backup, snapshot, restore). Acceptance: a new operator can stand up the 3-sequencer topology from the docs alone.

### What this means for timing

Original estimate in §4: **~6-8 weeks**. Revised estimate after audit: **~2-3 weeks**, dominated by Postgres provisioning + Caddyfile work + 24h prod soak. Coding is minimal; this is config + ops.

### What's still gated

Sub-issue 4 has a non-trivial decision embedded: managed Cloud SQL vs self-hosted Postgres on the same VPC. Tradeoffs:

- **Cloud SQL with regional HA.** Easy to operate. Costs ~$50-100/mo for a small instance with HA. Failover is GCP-managed. The Postgres becomes a separately-paid GCP-managed dependency.
- **Self-hosted on a small VM.** Cheaper (~$15/mo for the VM). We own the failure mode. Adds ops surface.

Default recommendation: **Cloud SQL** with regional HA, at least until pre-mainnet. Operating a single VM is fine until that VM becomes the SPOF that the multi-sequencer work was meant to remove. Revisit if cost becomes a constraint.

---

## 6. What changes for Option 3.2

This section is **scoping only**; no implementation in this PR.

The plumbing built in §5 (LeaderSchedule, failure detection, multi-instance deploy) carries over directly. What gets added on top:

- **Staking module (#50).** External validators bond `$AVOW`. Stake is the input to the weight function.
- **Disputes module (#51).** Slashing on equivocation, downtime, A1 (invalid attestation inclusion per PoUA §4.5).
- **Stake-weighted rotation.** `leader(slot)` changes from `validators[slot % N]` to weighted-random selection (Cosmos pattern). Determinism preserved via a chain-derived seed.
- **Operator onboarding.** Partner-recruitment process, key ceremony, initial stake distribution. Not engineering work but the gating dependency.
- **Validator-set membership transactions.** Permissioned in v1 (governance decides membership); permissionless in v3 (PoUA-gated entry).

The PoUA paper's §7 specifies the parameter set for the federated case. We do not need to re-derive any of that here; this doc is about the substrate that PoUA sits on, not PoUA itself.

---

## 7. Open questions

Things genuinely uncertain enough to flag rather than commit:

**Q1. Slot rotation cadence: per-slot or per-epoch?** Per-slot is simpler and gives the smoothest failover. Per-epoch (~4h, matching the PoUA epoch in §4.2) is more stable but worse for liveness if a leader dies mid-epoch. **Lean per-slot for v1 step 1.** Revisit if BFT-vote overhead becomes a bottleneck.

**Q2. Federated set size: 5, 7, or 10?** 5 is fragile (1/3 Byzantine = 2 nodes; small failure surface). 10 hits more partner-recruitment overhead. 7 is the comfortable middle (1/3 Byzantine = 2; 4 honest nodes is BFT-safe under any single combined failure). **Default 7, parameters tunable.** Decision deferrable until partner recruitment has shape.

**Q3. Slot-timeout for failure detection.** Start with 2× slot duration (so ~20s on current devnet cadence). Tune empirically once we have three nodes running and can measure proposal-latency distribution.

**Q4. Fork strategy for the Sovereign SDK pattern.** Upstream's preferred-sequencer pattern handles single-sequencer well; multi-sequencer support varies. If upstream lands a clean multi-sequencer abstraction in the next 6-8 weeks, we use it; if not, we extend our existing fork (`ligate-mainline`) with a thin LeaderSchedule trait. **Decision contingent on what upstream ships next.**

**Q5. Revisit triggers for the whole decision.**

- If PoUA gets de-scoped (not planning to), Option 3.3 (shared sequencer) becomes viable; reopen this doc.
- If Sovereign SDK ships a production-mature based-rollup pattern AND Celestia mainnet drops block time below 6s, Option 3.4 (based rollup) becomes viable for v2/v3; revisit.
- If partner recruitment for Option 3.2 stalls (zero operator commitments by month 4 of v1), revisit whether to ship Option 3.1 to mainnet as an honest interim.

---

## 8. Related work

- **#81 Force-include path.** Complementary censorship-resistance lever, valid regardless of which option we pick. Separate design doc; tracked there.
- **#50 Staking module.** Economic primitive for Option 3.2. Issue body has the module spec.
- **#51 Disputes module.** Slashing logic for Option 3.2. Cannot be designed without the validator-set model from this doc.
- **PoUA paper v0.7.2** (`ligate-research/papers/poua/poua.md`). §4 specifies the validator-weight function this substrate has to support. §7 specifies the parameter set. The paper assumes the substrate; this doc decides what that substrate is.
- **chain v1 milestone.** This decision sits on the critical path to mainnet alongside #50, #51, and the PoUA implementation stack (#126 umbrella).
