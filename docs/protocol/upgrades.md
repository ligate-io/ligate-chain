# Chain upgrades

This page explains how Ligate Chain handles changes to the protocol, the runtime, the parameters under governance, and the chain itself. Written for a reader who runs a node or builds against the protocol and needs to know **what changes break what**, and **how the chain coordinates the rare cases where they do**.

The default direction is the same one every credible chain takes: small changes are routine and non-breaking, big changes are explicit and announced. The mechanism for "explicit and announced" evolves across the chain's lifecycle — informal coordination with attestor orgs pre-mainnet, on-chain governance + upgrade modules post-mainnet.

---

## Two questions every upgrade has to answer

Before deciding what kind of upgrade something is, two questions:

1. **Does state continuity break?** Does any existing on-chain value (a `Schema`, an `AttestorSet`, an `Attestation`, a balance) become invalid, unreadable, or differently-interpreted after the change?
2. **Do existing transaction signatures still verify?** Does any signed-but-not-yet-included transaction become rejectable post-upgrade? Does any past attestation signature stop verifying?

If the answer to either is **yes**, the change is a **hard fork**: state continuity is broken, the chain id ladder advances ([protocol spec, "Chain id"](attestation-v0.md#chain-id)), every node restarts against new genesis, and any in-flight transactions have to be reconstructed.

If the answer to both is **no**, the change is a **soft fork**: existing state stays valid, existing signatures keep verifying, and the upgrade ships through normal release channels. Nodes upgrade at their own pace; old and new binaries co-exist on the network until everyone catches up.

---

## What counts as a soft fork

Changes that **do not** break state continuity or signature validity. Concretely:

- **Adding a new `CallMessage` variant at the end of the enum.** Borsh tags variants by declaration index. Appending a variant adds tag `N+1` for the new variant; existing tags `0..N` stay where they were. Old transactions deserialise unchanged; old signatures still verify against the unchanged `SignedAttestationPayload` shape.
- **Adding a new module to the runtime composition.** The new module's state lives in fresh keys; existing state is untouched. Nodes that don't yet know about the new module can still validate every old transaction.
- **Adding a new field on a private (non-public) struct.** Internal-only types are not part of the wire format.
- **Adding a new RPC endpoint.** Read-only additions; existing clients that don't call the new endpoint are unaffected.
- **Adding a new error variant** to `AttestationError`, `ClientError`, `GenesisError`. Public protocol enums are `#[non_exhaustive]` ([per #43](https://github.com/ligate-io/ligate-chain/issues/43)) so downstream consumers already include `_ => ...` arms.
- **Adding a new doc comment, README section, or test.** Trivially soft.

Soft forks ship in normal releases. The CI gates (`cargo audit`, the [Borsh snapshot tests](../../crates/modules/attestation/tests/borsh_snapshot.rs), the `#[non_exhaustive]` enum convention) catch the cases where a change you thought was soft is actually hard.

## What counts as a hard fork

Anything that changes how an existing on-chain value is **encoded, addressed, or interpreted**:

- **Adding, removing, or reordering fields** on `Schema`, `AttestorSet`, `Attestation`, `SignedAttestationPayload`, or any other Borsh-encoded protocol type. The Borsh layout snapshots in [`tests/borsh_snapshot.rs`](../../crates/modules/attestation/tests/borsh_snapshot.rs) fire red on every one of these.
- **Reordering existing `CallMessage` variants.** Borsh enum tags are positional; swapping two variants silently re-routes every transaction with the swapped tag.
- **Bumping a `MAX_*` constant** (`MAX_ATTESTOR_SET_MEMBERS`, `MAX_ATTESTATION_SIGNATURES`, `MAX_ATTESTOR_SIGNATURE_BYTES`). The `SafeVec<T, MAX>` type encodes its capacity in the type signature, and the Borsh derive emits a different `BorshDeserialize` impl per `MAX` value. A different `MAX` is a different wire format.
- **Changing the `SchemaId` / `AttestorSetId` derivation rule** (`SHA-256(owner ‖ name ‖ version)`, `SHA-256(sorted_members ‖ threshold)`). The derived id is content-addressed; changing the formula re-roots every existing schema and attestor set.
- **Changing the address format or the chain id integer** (`CHAIN_ID = 4242` in [`constants.toml`](../../constants.toml)). Both are inputs to transaction signature verification.
- **Changing the runtime composition** — adding a module, removing a module, switching a module's `Config` shape, or substituting a different DA / ZkVM spec into the canonical chain hash. Detected automatically by the `CHAIN_HASH` guard (next section).

Hard forks bump the chain-id trailing number per the locked ladder: `ligate-localnet` → unaffected (it's local-only), `ligate-devnet-1` → `ligate-devnet-2`, `ligate-testnet-1` → `ligate-testnet-2`, `ligate-1` → `ligate-2`. Genesis is regenerated. Every node operator restarts against the new genesis. Any in-flight transactions on the old chain are dropped — their signatures are domain-separated to the previous chain's hash and won't verify on the new one.

---

## The `CHAIN_HASH` guard

The chain has a built-in hard-fork detector: a constant called `CHAIN_HASH`, derived deterministically from the runtime's [universal-wallet `Schema`](https://github.com/Sovereign-Labs/sovereign-sdk) at build time. Implementation: [`crates/stf/build.rs`](../../crates/stf/build.rs); consumer: [`crates/stf/src/runtime_capabilities.rs`](../../crates/stf/src/runtime_capabilities.rs). Documented inline in `build.rs`:

> Used by the SDK's transaction authenticator to bind every signed tx to this exact runtime composition; clients on a different runtime composition produce signatures that won't verify.

Practical effect: every transaction's signature includes `CHAIN_HASH` in its domain-separation tag. If a node upgrade shifts the runtime schema (any module added, removed, or with a changed Config / state-item shape), `CHAIN_HASH` changes. From that moment, transactions signed against the old hash fail verification on the new node. There's no possibility of a node "accidentally" running with a subtly different runtime than its peers — they either agree on the hash or they don't transact.

This is the reason the chain doesn't need a separate "is this binary the right binary" check: the hash is the check. It's also the reason a hard fork is a real reset rather than a continuous upgrade — you can't roll a runtime change forward through `cargo update` and pretend nothing happened.

The hash is reproducible across machines because the `Schema` is derived from canonical Rust types via a deterministic hasher, with the spec parameters pinned to `MockDaSpec + MockZkvm + MultiAddressEvm` for hash purposes regardless of the binary's actual DA / ZkVM / address spec at runtime. Two operators on `ligate-devnet-1` get byte-identical `CHAIN_HASH` outputs, even if one runs against MockDa locally and the other against Celestia mocha.

---

## Pre-mainnet: coordinated redeploy

Today (and through devnet, testnet, and any pre-mainnet phase), upgrades are **socially coordinated** between Ligate Labs and the attestor org operators.

### Soft-fork release flow (most changes)

1. Land the change in a PR. CI gates (`fmt`, `clippy`, `check`, `test`, `doc`, `audit`, plus the Borsh snapshot tests) catch the obvious soft-vs-hard-fork misclassifications.
2. Merge to `main`. Tag a release at the new commit (when releases start happening — pre-tag-1 it's just "the latest commit on main").
3. Operators pull, rebuild, restart their nodes at their own pace. The chain stays live across the transition; old and new binaries interop because the wire format hasn't changed.

There is no on-chain "upgrade ceremony" for soft forks. The release is the upgrade.

### Hard-fork release flow (rare, deliberate)

1. Decide the hard fork is necessary. Document why in an issue and link from the PR.
2. Coordinate the new genesis with attestor org reps. Bump the chain id (e.g. `ligate-devnet-1` → `ligate-devnet-2`).
3. Snapshot any state that should carry over (often: nothing; a hard fork is usually paired with a decision to drop all existing state). If state is migrating, write a one-off migration script and check it in as a tagged release artifact.
4. Set a coordinated cutover time. Every operator restarts their node against the new chain id, the new binary, and (if applicable) the new genesis at the agreed cutover.
5. The old chain stops producing blocks (sequencer is halted at the cutover); the new chain starts at height 0 with the new genesis.

This is informal because it has to be. Pre-mainnet, there are no real funds at stake, no economic finality assumptions, and the operator pool is small enough to coordinate on a chat channel. Post-mainnet, this informality stops scaling and the upgrade module takes over (next section).

---

## Post-mainnet: governance + upgrade modules

Two modules will land in v1 to formalise upgrade flow:

### Governance module ([#41](https://github.com/ligate-io/ligate-chain/issues/41))

On-chain proposals + voting + execution for **parameter** changes. Initial scope:

- `attestation_fee`, `schema_registration_fee`, `attestor_set_fee` (fees in `$LGT`)
- `max_builder_bps` cap (default 5000 = 50%)
- Treasury address
- Later: attestor set rotation policy, slashing thresholds (when [`disputes`](https://github.com/ligate-io/ligate-chain/issues/51) lands)

Mechanism: a proposal includes the new value; `$LGT` stakers vote; if it passes the configured threshold, the parameter updates **without a binary upgrade**. The mechanism for this is the v0 hygiene work tracked in [#40](https://github.com/ligate-io/ligate-chain/issues/40) — moving governance-tunable constants from compile-time `const` to on-chain state — which is a precondition for governance to do anything.

Governance proposals are **not** hard forks. They don't change `CHAIN_HASH`. Existing signatures still verify. The chain id stays the same. They're the chain saying "use this new value going forward" without anyone restarting their node.

### Upgrade module ([#42](https://github.com/ligate-io/ligate-chain/issues/42))

Coordinated **code-level** upgrades following Cosmos SDK `x/upgrade` semantics. Lets governance (or hardcoded ops keys for the v1-v1.5 transition) schedule a runtime swap at block height N. Every node operator deploys the new binary before N. At N, the chain switches to the new STF.

This is what bridges soft-fork deploys (anyone updates whenever) and hard-fork resets (everyone halts). Specifically, the upgrade module makes it possible to ship a change that **would** be a hard fork under the build.rs `CHAIN_HASH` check, by:

1. Building the new binary with the new runtime schema (and a new `CHAIN_HASH`).
2. Pre-publishing the new binary with a known `CHAIN_HASH` value.
3. Scheduling the upgrade at height N via a governance proposal.
4. At N, the chain's runtime authenticator switches from validating signatures against the old hash to validating against the new hash. Operators that haven't upgraded fail to apply blocks past N — they self-halt rather than fork.

The upgrade module is the v1 path forward. Until it lands, hard forks are the pre-mainnet coordinated-redeploy flow above.

---

## Roadmap: bringing researched primitives to the chain

Ligate maintains a public research repository at [`ligate-research`](https://github.com/ligate-io/ligate-research) where protocol-level primitives are designed in working-paper form before they land in the chain. This section maps each planned primitive to the chain-side mechanism that brings it in, and explains the staging logic that makes the rollout tractable.

### The two-flavor pattern

Adding a researched primitive to the chain is operationally one of two things, depending on where we are in the chain-id ladder:

- **Pre-mainnet (now through v2):** new primitive ships as a new module in the runtime composition, paired with a chain-id bump. Fresh genesis, no state migration, operators redeploy. This is the easy case and we use it freely.
- **Post-mainnet (v3+):** new primitive must either be baked into mainnet from genesis, or scheduled via the upgrade module ([#42](https://github.com/ligate-io/ligate-chain/issues/42)) at a coordinated block height. The latter is real work and is reserved for things we genuinely could not anticipate at mainnet launch.

The strategic implication: **bake every known primitive into mainnet from day 1.** Use chain-id bumps freely while pre-mainnet to integrate primitives as their research papers stabilize. Reserve post-mainnet upgrades for unknown future work.

### Planned primitives and where they land

| Paper | Lands in | Mechanism | Chain-id transition |
|---|---|---|---|
| **PoUA** (consensus weighting) | v3 mainnet from genesis | Module + warmup-then-activation soft fork | `ligate-testnet-1` → `ligate-1` |
| **Native delegation** (Iris foundation) | v0.5 module | Chain-id bump | `ligate-devnet-1` → `ligate-devnet-2` |
| **Per-schema fee markets** | v1 module | Chain-id bump | `ligate-devnet-2` → `ligate-testnet-1` |
| **Cross-schema composition** | Post-mainnet, demand-driven | Upgrade module ceremony | Within `ligate-1` |
| **Time-locked / commit-reveal attestations** | Post-mainnet, demand-driven | Upgrade module ceremony | Within `ligate-1` |

The first three land pre-mainnet. By the time `ligate-1` mainnet starts producing blocks, the runtime composition includes the `reputation` module (PoUA), the `delegation` module, and the per-schema fee market machinery. None of these require a post-mainnet hard fork because they're already in the genesis-time runtime.

The last two are reserved for post-mainnet only if a real use case emerges. Cross-schema composition needs partner demand we have not yet collected. Time-locked attestations are niche enough that we should not pre-commit runtime real estate for them.

### PoUA specifically: how it activates without a hard fork

PoUA is the most subtle of the planned primitives because it changes consensus weighting, which would normally be a hard-fork-class change. The design avoids that:

1. **Mainnet genesis includes the `reputation` module** with its full state, update worker, REST endpoints, and slashing logic.
2. **Mainnet starts in warmup**: validators run with `weight = stake` (legacy stake-only PoS) for the first $T_{\text{warmup}}$ epochs (recommended 14 epochs, ≈2.3 days). Reputation accumulates per the §4.3 update function but does not enter vote weight.
3. **Activation is a single governance transaction** at the end of warmup, flipping a `reputation_active: bool` flag in module state from `false` to `true`.
4. **At the next block boundary after activation**, every validator computes `weight = stake × reputation` and the BFT vote tally uses the new weighting.

This is a soft fork class change because the activation does not change wire format, signature shape, or runtime composition. It changes a single state value. The flag-flip mechanism is the same one used for any governance-controlled boolean parameter.

The economic activation timing matters: validators present at warmup-end carry their accumulated reputation into the post-warmup phase. Validators who join after warmup begin at $r_{\min}$. This gives a ~2-3 day window during which reputation distributions are forming under uniform conditions, with no opportunity for early-arrivals to lock in disproportionate consensus power before the rest of the field has caught up.

Full mechanism specification is in the [PoUA working paper §4.6 and §7.4](https://github.com/ligate-io/ligate-research/blob/main/papers/poua/poua.pdf).

### What if we want a primitive we did not anticipate?

The post-mainnet path uses the upgrade module ([#42](https://github.com/ligate-io/ligate-chain/issues/42)). The mechanics, in the order they happen:

1. **Engineering**: implement the new module on a testnet branch. Run for ≥2 months on a parallel testnet to surface bugs before mainnet exposure.
2. **Governance**: file a proposal with the new module's code reference, the activation block height, and the migration story (if any state migrates).
3. **Validator binary distribution**: every node operator pulls the new binary and loads it before the activation block. The upgrade module enforces this at the binary level: an unupgraded node will fail to apply blocks past the activation height and will self-halt.
4. **Migration**: any new state fields get default values via the genesis-config-style hooks we already use for module initialization. Existing state is untouched.
5. **Activation**: at block N, the new module's logic kicks in. No state break, no chain-id change.

This is tractable but expensive — multi-month coordination, ecosystem-wide tooling updates (wallets, indexers, explorers all need awareness of the new module's events). Cosmos chains do this 2-3 times per year at maturity. Plan for that cadence post-mainnet.

### Why this discipline matters

A chain that adds primitives post-mainnet via routine hard forks accumulates two costs over time. The first is coordination overhead: every ecosystem participant has to track upgrade schedules. The second is fragmentation: validators that don't upgrade lag the network; wallets that don't upgrade misrepresent state to users.

The strategy of "bake known primitives into mainnet from day 1" trades pre-mainnet engineering effort for post-mainnet operational simplicity. We do more work in the v0-v2 phase, ship a richer mainnet, and minimize the rate at which the ecosystem has to absorb breaking changes once real value flows.

---

## Coordination with Celestia DA

We use Celestia for data availability. Celestia upgrades on its own cadence; we are a consumer.

Two relationships matter:

1. **DA layer upgrades do not require a Ligate upgrade.** Celestia's mainnet rolls forward — new validator set, new consensus version, etc. Ligate Chain reads whatever blobs land in our namespace; the DA layer's internal evolution is opaque to us as long as the namespace contract holds.
2. **Ligate-side hard forks do not require a DA-layer change.** A new genesis on `ligate-devnet-2` writes blobs to the same Celestia namespace. Old blobs still exist on Celestia (immutable history) but our STF treats them as outside-genesis pre-history.

Practically: a Celestia testnet rotation (e.g. mocha → arabica → mocha-2) **is** a hard fork from our perspective if we're forced off the namespace, because every operator's `celestia.toml` has to change endpoint and re-sync. That's a genesis-restart event by definition. But a within-Celestia upgrade (consensus bump, slashing rule change, fee adjustment) doesn't touch us.

---

## Wire-format protection layered into CI

Defense in depth against accidental hard forks:

1. **`#[non_exhaustive]` on public enums** ([#43](https://github.com/ligate-io/ligate-chain/issues/43), shipped via PR #103). Forces downstream `match` expressions to include `_ => ...`, so adding a `CallMessage` variant doesn't break compiles in third-party crates that consume our types. The attribute is annotation-only — it does not affect Borsh layout.
2. **Borsh snapshot tests** ([#45](https://github.com/ligate-io/ligate-chain/issues/45), shipped via PR #108). 13 snapshots in [`tests/borsh_snapshot.rs`](../../crates/modules/attestation/tests/borsh_snapshot.rs) covering every public protocol type. A PR that changes the encoding of `Schema`, `AttestorSet`, `Attestation`, `SignedAttestationPayload`, or any `CallMessage` variant fails CI. The diff makes the change explicit, the file's docstring tells the dev to either revert or document.
3. **`cargo audit` on every PR.** Catches transitive vulns in the dep graph. Ignored advisories live in [`audit.toml`](../../audit.toml) with rationale.
4. **`CHAIN_HASH` derivation** in [`crates/stf/build.rs`](../../crates/stf/build.rs). The runtime-level catch: a build that produces a different schema produces a different hash, which fails signature verification at runtime. Ultimate backstop.

A change that survives all four layers is a soft fork.

---

## What this protects against, and what it doesn't

**Protects against:**

- A field-rename PR that ships an unintended Borsh layout shift — Borsh snapshot fires.
- A `MAX_*` cap bump that nobody noticed changes the wire envelope — Borsh snapshot fires.
- Two operators running subtly different runtime composition (one has the new module, one doesn't) — `CHAIN_HASH` mismatch, signatures don't verify, the operator without the new module can't apply blocks past the upgrade height.
- A `CallMessage` reorder that would silently re-route every transaction — Borsh snapshot fires (because the variant tag is positional).
- A new advisory on a transitive dep that creates a real exploit path — `cargo audit` fires.

**Does not protect against:**

- A semantic change that doesn't shift bytes (e.g. changing what `fee_routing_bps = 0` *means*, while keeping the field at the same offset and the same type). The Borsh snapshot doesn't fire because the encoding is unchanged. Detection here is human review of the relevant module's call handlers and tests.
- A logic bug in the state transition. Tested by the integration suite ([29 tests across `attestation::tests`](../../crates/modules/attestation/tests/integration.rs)), but absence of a test for a specific edge case is always possible.
- A vulnerability in the SDK at our pinned rev that we don't know about. Mitigated by the SDK's own advisory process, which we'd pick up on the next pin bump.
- An attestor org compromising its own signing key. Out of scope for chain protection; that's the attestor's operational concern, documented in the [devnet runbook §7](../development/devnet.md).

For the full v0 attacker model and every mitigation across attestation, sequencer, wire-format, primitive, and operational layers, see [`threat-model.md`](threat-model.md).

---

## What this document does **not** cover, deliberately

- **The mechanics of the upgrade module's wire shape.** The proposal type, voting period, threshold, execution height — all of that lands in #42. This page is the meta-level "how does the chain handle change" view; the per-proposal-type spec is in the upgrade module itself once that ships.
- **State migration tooling.** Pre-mainnet hard forks drop state; post-mainnet hard forks would migrate. Migration tooling (a `ligate-migrate` binary, schema-diff helpers, replay-from-DA support) is downstream work, tracked separately when the first real migration is needed.
- **Validator-set governance.** Sovereign rollups don't have a validator set in the L1 sense; the sequencer is the analogous role, and decentralising the sequencer is its own concern ([#82](https://github.com/ligate-io/ligate-chain/issues/82)). Attestor sets are application-level, not consensus-level.
- **Audit-trigger thresholds.** When does a hard fork warrant a fresh security audit, and what's the disclosure window for known-vulnerable releases? That's an operational policy that lands closer to mainnet, not a protocol-level spec.

---

## Quick reference

| Change | Soft / hard | Detected by | Procedure |
|---|---|---|---|
| Append `CallMessage` variant | Soft | Borsh snapshot stays green; `#[non_exhaustive]` already in place | Normal release |
| Reorder `CallMessage` variants | Hard | Borsh snapshot fires | Coordinated reset, chain-id bump |
| Add field to `Schema` | Hard | Borsh snapshot fires; `CHAIN_HASH` shifts | Coordinated reset |
| Add module to runtime | Soft (logically) but hard (cryptographically) | `CHAIN_HASH` shifts | Upgrade-module ceremony when v1 lands; coordinated reset before that |
| Add new RPC endpoint | Soft | None | Normal release |
| Bump `MAX_ATTESTOR_SET_MEMBERS` | Hard | Borsh snapshot fires (different `SafeVec` cap) | Coordinated reset |
| Change `attestation_fee` value | Soft once #40 + #41 ship | None (deliberate) | Governance proposal post-mainnet |
| Change `attestation_fee` type (`Amount` → `u128` say) | Hard | Borsh snapshot fires | Coordinated reset |
| Change attestor signature scheme | Hard | Every existing signature breaks | Coordinated reset, schema version bump |
| Bump Sovereign SDK rev | Soft if `CHAIN_HASH` unchanged, hard otherwise | `CHAIN_HASH` derivation; review the SDK changelog | Test build, check the hash, then either soft-release or coordinated-reset |
| Switch Celestia testnet (mocha → mocha-2) | Hard (operationally) | `celestia.toml` change | Coordinated reset (every operator re-syncs from the new namespace) |

---

## Related

- [Protocol spec, "Chain id"](attestation-v0.md#chain-id) — the locked four-row identifier ladder
- [`crates/stf/build.rs`](../../crates/stf/build.rs) — `CHAIN_HASH` derivation
- [`crates/stf/src/runtime_capabilities.rs`](../../crates/stf/src/runtime_capabilities.rs) — where `CHAIN_HASH` plugs into the runtime authenticator
- [`tests/borsh_snapshot.rs`](../../crates/modules/attestation/tests/borsh_snapshot.rs) — the wire-format guard
- [`audit.toml`](../../audit.toml) + [`.github/workflows/ci.yml`](../../.github/workflows/ci.yml) — `cargo audit` gate
- Issues [#40](https://github.com/ligate-io/ligate-chain/issues/40) (constants → state), [#41](https://github.com/ligate-io/ligate-chain/issues/41) (governance), [#42](https://github.com/ligate-io/ligate-chain/issues/42) (upgrade module), [#43](https://github.com/ligate-io/ligate-chain/issues/43) closed, [#45](https://github.com/ligate-io/ligate-chain/issues/45) closed, [#54](https://github.com/ligate-io/ligate-chain/issues/54) chain-id ladder
