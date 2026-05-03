# DA layers: keeping the swap cheap

Ligate Chain is built on the Sovereign SDK, which is generic over the data-availability layer. Today the rollup binary supports two DA flavours (Mock, Celestia); the protocol itself is generic over `DaSpec` and could swap to Ethereum blobs, EigenDA, Avail, Near DA, or a future Ligate-native DA without touching the STF, modules, or client crates.

This page exists for three reasons:

1. **Document the property** so reviewers and operators know the seam exists.
2. **Defend the property in CI** so an accidental coupling doesn't slip in under deadline pressure.
3. **Give a worked playbook** for adding a new DA adapter when the time comes.

Tracking issue: [#117](https://github.com/ligate-io/ligate-chain/issues/117).

## Why we care

Two structural reasons:

1. **Vendor risk.** Celestia is the right pick for v0 devnet (working SDK adapter, low-friction integration, alive ecosystem). The DA landscape between now and mainnet is in flux. If Celestia's roadmap or economics shift, we want the swap to be a 2-week project, not a 6-month rewrite.
2. **Sovereignty.** Ligate is a general-purpose attestation protocol, not a Celestia rollup. The protocol layer (`crates/stf`, `crates/modules/attestation`, `crates/client-rs`) has no business knowing about a specific DA backend. Keeping the seam clean today preserves the long-term option of a Ligate-native DA without committing to building one.

What this page is **not**:

- A commitment to swap. Celestia stays the devnet DA.
- A plan to build a Ligate-native DA. Greenfield DA is a multi-engineer multi-year effort and not a side quest.
- A mainnet DA decision. That happens closer to mainnet when the modular-DA landscape is clearer.

The point is to make the option cheap, not to take it.

## Architecture

The Sovereign SDK's `Spec` trait carries the DA layer transitively:

```rust
pub trait Spec {
    type Da: DaSpec;
    type InnerZkvm: Zkvm;
    type OuterZkvm: Zkvm;
    type Address;
    type ExecutionMode;
    type CryptoSpec;
    type Storage;
}
```

Our STF, modules, client, and tests are generic over `S: Spec`, so they receive the DA layer indirectly. They never name a concrete DA type. The rollup binary is where `S` gets pinned to a concrete spec, which in turn pins `Da` to `MockDaSpec` or `CelestiaSpec`.

This gives us **two clean layers**:

| Layer | DA awareness | Examples |
|---|---|---|
| **Protocol** (DA-agnostic by design) | None. Generic over `S: Spec`. | `crates/stf`, `crates/stf-declaration`, `crates/modules/attestation`, `crates/client-rs` |
| **Rollup** (DA-bound by design) | Pins `S` to a concrete spec carrying a concrete `Da`. | `crates/rollup/src/celestia_rollup.rs`, `crates/rollup/src/mock_rollup.rs`, the Risc0 guest under `provers/risc0/guest-celestia/` |

The CLI dispatch in `crates/rollup/src/main.rs` picks which blueprint to instantiate based on `--da-layer mock|celestia`. Adding a new DA layer means adding a new module under `crates/rollup/src/` and a new arm to the CLI; nothing in the protocol layer changes.

## Inventory: where DA boundaries live

As of 2026-05-03, every reference to a concrete DA backend in this repo lives in one of the following locations. **Anything outside this list is a coupling violation.**

### Allowed (DA-bound by design)

```
crates/rollup/src/main.rs                        # --da-layer mock|celestia CLI dispatch
crates/rollup/src/lib.rs                         # re-exports both blueprint impls
crates/rollup/src/mock_rollup.rs                 # MockLigateRollup + MockRollupSpec
crates/rollup/src/celestia_rollup.rs             # CelestiaLigateRollup + CelestiaRollupSpec
crates/rollup/Cargo.toml                         # sov-celestia-adapter + sov-mock-da deps
crates/rollup/tests/devnet_config.rs             # config-loader tests for both flavours
crates/rollup/provers/risc0/build.rs             # guest crate name pin
crates/rollup/provers/risc0/Cargo.toml           # guest path declaration
crates/rollup/provers/risc0/guest-celestia/      # DA-specific Risc0 guest sub-crate
```

### Forbidden (must stay DA-agnostic)

```
crates/stf/                                      # zero references
crates/stf-declaration/                          # zero references
crates/modules/                                  # zero references
crates/client-rs/                                # zero references
```

The handful of `namespace` substring matches in `crates/modules/attestation/` are unrelated; they refer to **schema namespaces** (`themisra.proof-of-prompt`), not Celestia's `sov_celestia_adapter::Namespace`. The CI guardrail (below) is case-insensitive on `celestia` itself, which avoids the false positive.

## CI guardrail: `scripts/check-da-isolation.sh`

Runs as a required `CI pass` job (see [`.github/workflows/ci.yml`](../../.github/workflows/ci.yml)). Greps the four DA-agnostic crate trees for `celestia` and `sov_celestia_adapter` (case-insensitive), fails CI on a hit. Lightweight and noise-free; does not match `Namespace` alone (avoids the schema-namespace false positive).

What the guardrail catches:

- A stray `use sov_celestia_adapter::*` in a module crate
- A `CelestiaSpec` type alias added to the STF
- A new test under `crates/modules/` that imports `CelestiaService`
- A doc comment that mentions Celestia by name in a DA-agnostic crate (yes, even prose; we hit this once during the [#119](https://github.com/ligate-io/ligate-chain/issues/119) fuzz PR and rephrased the comment to use "DA blob" instead)

What it doesn't catch (and we accept):

- A new DA backend coupling that uses different naming (e.g. someone adds Avail-specific code without tripping the `celestia` regex). Extending the guardrail is one-line cheap when we add a new adapter.
- Conceptual coupling: imports that aren't named after a DA backend but assume Celestia's ordering or finality semantics. Code review territory.

When you add a new DA adapter (see playbook below), update the guardrail to grep for the new adapter's name too.

## Playbook: adding a new DA adapter

Worked example: imagine adding **Avail** as a third DA flavour. Steps and approximate effort:

### 1. Add the SDK adapter dep

```toml
# Cargo.toml (workspace dependencies)
sov-avail-adapter = { git = "https://github.com/Sovereign-Labs/sovereign-sdk.git", rev = "..." }
```

If the SDK doesn't ship an adapter yet, this is a multi-week prerequisite (write the adapter against `DaSpec`/`DaService`/`DaVerifier` traits, possibly upstream it). Out of scope for this playbook; assume the adapter exists.

### 2. Create `crates/rollup/src/avail_rollup.rs`

Copy [`celestia_rollup.rs`](../../crates/rollup/src/celestia_rollup.rs) as the template. Replace:

- `CelestiaSpec` -> `AvailSpec` (or whatever the adapter's `DaSpec` impl is named)
- `CelestiaVerifier` -> `AvailVerifier`
- `CelestiaService` -> `AvailService`
- The `RollupParams` struct contents (Avail's params will differ; check the adapter's docs)
- The `ROLLUP_BATCH_NAMESPACE` constant if the new DA uses a namespace concept; otherwise drop it

Keep the same `RollupBlueprint` + `FullNodeBlueprint` trait impls. Same `ligate_stf::Runtime<S>`, same `MultiAddressEvm` address shape. The protocol layer doesn't change.

### 3. Wire the new blueprint into the CLI

```rust
// crates/rollup/src/main.rs
match args.da_layer {
    DaLayer::Mock => MockLigateRollup::run(...),
    DaLayer::Celestia => CelestiaLigateRollup::run(...),
    DaLayer::Avail => AvailLigateRollup::run(...),  // new arm
}
```

Add the `Avail` variant to the `DaLayer` enum, update the CLI help text, regenerate any associated docs.

### 4. Re-export from the rollup library

```rust
// crates/rollup/src/lib.rs
mod avail_rollup;
pub use avail_rollup::{AvailLigateRollup, AvailRollupSpec};
```

### 5. (Optional) Add a Risc0 guest for the new DA

If we want real ZK proofs against Avail (Phase A.4 work), copy `crates/rollup/provers/risc0/guest-celestia/` to `guest-avail/`, swap the `sov-celestia-adapter` import for `sov-avail-adapter`. Update [`build.rs`](../../crates/rollup/provers/risc0/build.rs) to register the new guest. Pre-A.4, skip this step entirely (mock zkVM is fine).

### 6. Update integration tests

[`crates/rollup/tests/devnet_config.rs`](../../crates/rollup/tests/devnet_config.rs) loads a per-DA config fixture. Add an `avail.toml` flavour and a corresponding test case.

### 7. Extend the CI guardrail

```bash
# scripts/check-da-isolation.sh
patterns=(
  "celestia"
  "sov_celestia_adapter"
  "avail"           # new
  "sov_avail_adapter"  # new
)
```

This is the line that defends "DA-agnostic by construction" going forward. Without it, an Avail leak into `crates/modules/` would compile silently.

### 8. Document the new flavour

Update `devnet/README.md` boot instructions, add an entry to `docs/development/devnet.md`'s flavour table, refresh this page's inventory section (the "Allowed" block grows by a few entries).

### Total effort

Steps 1-4, 6, 7, 8 fit in a single PR if the SDK adapter exists and is feature-complete; rough estimate 2-3 days of focused work plus one round of review. Step 5 (guest crate) is +1-2 weeks if Phase A.4 is active.

Steps that **don't** appear in this playbook are exactly the ones we want to keep out: no STF changes, no module changes, no `Spec` changes, no client-rs changes. If a step turns out to require any of those, the protocol layer has accidentally absorbed DA-specific assumptions and the seam needs repair before the new adapter ships.

## Audit findings (2026-04-30)

A full grep of `crates/` for `celestia`, `CelestiaSpec`, `CelestiaService`, `Namespace`, `tia` confirmed:

- Coupling is contained entirely within `crates/rollup/` and its prover sub-crates.
- Zero leaks in `crates/stf/`, `crates/stf-declaration/`, `crates/modules/`, `crates/client-rs/`.
- The `namespace` substring matches in `crates/modules/attestation/` are schema-namespace prose, unrelated to Celestia's `Namespace` type.

The audit re-runs automatically every PR via [`scripts/check-da-isolation.sh`](../../scripts/check-da-isolation.sh). New leaks fail CI before they merge.

## Out of scope (separate decisions)

- **Building a homegrown DA layer.** Multi-engineer multi-year effort. Keeping the option open here, not committing to it.
- **Switching off Celestia for devnet.** No reason. Celestia works, the SDK adapter is maintained, swapping costs nothing now and has no upside for Q2 2026.
- **Picking the production DA for mainnet.** That decision happens closer to mainnet when the modular-DA landscape settles. The audit + playbook here just make sure picking is cheap when we get there.

## Related

- [`upgrades.md`](upgrades.md): full hard-fork story; DA swap is one of the categories that bumps the chain id.
- [`threat-model.md`](threat-model.md): DA layer in the trust posture (we trust Celestia for ordering + availability today; a swap inherits a different trust set).
- [#119](https://github.com/ligate-io/ligate-chain/issues/119): fuzz coverage for byte boundaries; complementary to this issue's grep guardrail.
- [#117](https://github.com/ligate-io/ligate-chain/issues/117): tracking issue for this page.
