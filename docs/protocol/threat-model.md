# Ligate Chain threat model (v0)

The single page that says what an attacker can and cannot do against the chain today, what protections exist, where they live in code, and what is deliberately not modeled yet.

Audience: external auditors, design partners, security researchers, and our own future selves.

## What this document is not

- A formal threat model in the STRIDE or PASTA sense. Plain-English coverage is what v0 needs.
- Pen-test results. This document ships the model; the audit is a separate workstream.
- A claim of comprehensive coverage. We list what we have thought through. Where we are blind, we say so.

For incident reporting and coordinated disclosure, see [`SECURITY.md`](../../SECURITY.md). For the chain-id ladder and what counts as a hard fork, see [`upgrades.md`](upgrades.md).

## Trust posture across the chain-id ladder

The threat surface evolves with the chain-id ladder defined in [`attestation-v0.md` §Chain id](attestation-v0.md#chain-id):

| Environment | Chain id | Trust posture | What is federated |
|---|---|---|---|
| Local single-node dev | `ligate-localnet` | Operator trusts their own machine | Everything |
| Public devnet | `ligate-devnet-1` | Permissioned sequencer + federated attestor sets, no economic finality | Sequencer (Ligate Labs), attestor sets per schema |
| Public testnet | `ligate-testnet-1` | Permissioned sequencer + federated attestor sets + slashing | Sequencer + attestor staking module |
| Mainnet | `ligate-1` | Full PoUA + disputes + multi-sequencer | None at the protocol layer |

This document describes the **v0** posture (`ligate-localnet`, `ligate-devnet-1`). Each subsequent ladder rung tightens specific gaps named below. v3 (`ligate-1`) closes the consensus-weighting gap via [PoUA](https://github.com/ligate-io/ligate-chain/issues/126).

## How to read this

Each entry below names: **what the attacker tries to do**, **what protects against it in v0**, **where the protection lives in code**, and **what would harden it later** (with tracking issue).

---

## 1. Attestation-layer adversaries

These attack the protocol primitive itself: schemas, attestor sets, attestations.

### 1.1 Replay attacker

**Attack:** re-submit a captured signed payload to inflate counts or deceive a verifier.

**Defense:** write-once invariant on `(schema_id, payload_hash)`. The same pair cannot be submitted twice, so a captured signature cannot be re-anchored.

**Where:** invariant 5 in [`attestation-v0.md` §Invariants](attestation-v0.md#invariants); enforced in `crates/modules/attestation/src/lib.rs` on the `SubmitAttestation` handler.

**Cross-chain replay:** every signed tx is domain-separated by `CHAIN_ID = 4242` (per `constants.toml`) plus the `CHAIN_HASH` derived from the runtime composition. Signatures from `ligate-devnet-1` do not verify on `ligate-devnet-2`.

### 1.2 Schema squatter

**Attack:** register the canonical name of an upcoming application to extort or mislead users.

**Defense:** `schema_id = SHA-256(owner ‖ name ‖ version)`. Two different owners registering the same name produce two different ids. There is no protocol-level "real" owner of an arbitrary name.

**Where:** `Schema::derive_id` in `crates/modules/attestation/src/lib.rs`; documented in [`attestation-v0.md` §Entities](attestation-v0.md#entities).

**Future hardening:** off-chain verified-tag metadata in SDKs and explorers, the same model as ERC-20 token names. Not a protocol-level fix because there is no canonical answer to "who owns the name".

### 1.3 Malicious schema owner (fee skimming)

**Attack:** set `fee_routing_bps` to 100% to route every fee paid by users to a private address.

**Defense:** protocol caps `fee_routing_bps` at 5000 (50%). The remaining fraction always flows to the treasury.

**Where:** invariant 2 in [`attestation-v0.md` §Invariants](attestation-v0.md#invariants); enforced at `RegisterSchema` validation in `crates/modules/attestation/src/lib.rs`.

**Future hardening:** governance ([#41](https://github.com/ligate-io/ligate-chain/issues/41)) can adjust the cap; the cap itself moves to state ([#40](https://github.com/ligate-io/ligate-chain/issues/40)) so it is governance-tunable without a hard fork.

### 1.4 Compromised attestor (single signer)

**Attack:** sign a fraudulent attestation under a federated AttestorSet using one corrupted key.

**Defense:** AttestorSet enforces an `M-of-N` threshold. A single compromised key cannot meet the threshold alone.

**Where:** invariant 4 in [`attestation-v0.md` §Invariants](attestation-v0.md#invariants); signature batch verification in `crates/modules/attestation/src/lib.rs`.

**Future hardening:** disputes module ([#51](https://github.com/ligate-io/ligate-chain/issues/51)) adds bonded stake plus slashing on successful fraud proof. v1.

### 1.5 Colluding attestor majority

**Attack:** coordinate among threshold members of an AttestorSet to sign a falsehood.

**Defense in v0:** **not protocol-prevented.** The set's social and reputational stake is the deterrent. Schema consumers choose which AttestorSets to trust based on the operator's identity, history, and off-chain reputation.

**Defense in v1:** [#51](https://github.com/ligate-io/ligate-chain/issues/51) disputes module. Successful fraud proof slashes the AttestorSet's bonded stake. Bounty paid to the disputer. Economic asymmetry makes collusion increasingly expensive as bonded stake grows.

**Where:** v0 trust model documented in [`attestation-v0.md` §Trust model](attestation-v0.md#trust-model).

### 1.6 Spam attacker

**Attack:** flood the chain with worthless attestations to degrade availability or storage.

**Defense:** per-attestation fee (default 0.001 `$LGT`) plus one-time registration fees (`SCHEMA_REGISTRATION_FEE` 100 `$LGT`, `ATTESTOR_SET_FEE` 10 `$LGT`). Costs scale linearly with volume.

**Where:** [`attestation-v0.md` §Fees](attestation-v0.md#fees); enforced via `sov-bank` integration in `crates/modules/attestation/src/lib.rs`.

**Future hardening:** per-schema fee markets ([#138](https://github.com/ligate-io/ligate-chain/issues/138)) raise fees on high-demand schemas dynamically. v3 mainnet-genesis primitive.

### 1.7 Privacy attacker (payload recovery)

**Attack:** recover prompt or output content from on-chain attestations.

**Defense:** the chain stores `payload_hash` (32 bytes SHA-256) and signatures only. Plaintext payloads never touch the chain. Recovering a payload requires inverting SHA-256, which is infeasible for non-trivial payloads.

**Where:** the data model in [`attestation-v0.md` §Entities](attestation-v0.md#entities); the canonical signed-payload definition in [`attestation-v0.md` §Signed payload for attestations](attestation-v0.md#signed-payload-for-attestations).

**Future hardening:** schemas that need stronger privacy can use Pedersen or zk-friendly commitments before hashing, with the protocol unaffected. Schema-level concern, not a chain-level change.

---

## 2. Sequencer-layer adversaries

The single permissioned sequencer is a v0 trust assumption. v1 hardens this.

### 2.1 Censorship by sequencer

**Attack:** the sequencer refuses to include valid attestations from a specific submitter or schema.

**Defense in v0:** **not protocol-prevented.** Censorship is possible but auditable. Clients can observe the gap between submitting and inclusion and detect missing attestations against the public mempool.

**Defense in v1:** [#81](https://github.com/ligate-io/ligate-chain/issues/81) force-include path. A submitter can bypass the sequencer by writing the transaction directly to Celestia. The runtime treats force-included transactions as authoritative regardless of sequencer cooperation.

**Defense in v3 mainnet:** PoUA ([#126](https://github.com/ligate-io/ligate-chain/issues/126)) reputation update penalizes selective censorship via the A2 detector ([§4.5](https://github.com/ligate-io/ligate-research/blob/main/papers/poua/poua.pdf), Appendix A.1) once enough traffic exists to calibrate.

**Where:** v0 single-sequencer architecture documented in [`devnet/README.md`](../../devnet/README.md) and [`devnet.md` §5](../development/devnet.md).

### 2.2 Sequencer DoS

**Attack:** crash, exhaust, or partition the single sequencer to halt the chain.

**Defense in v0:** **not protocol-prevented.** Single-sequencer means single point of failure for liveness. Operational mitigation only (monitoring, alerting, fast restart).

**Defense in v1:** [#82](https://github.com/ligate-io/ligate-chain/issues/82) multi-sequencer / leader rotation. Validators rotate proposer duty so any single failure does not halt the chain.

### 2.3 Sequencer key compromise

**Attack:** the sequencer's signing key is stolen and used to produce malicious blocks.

**Defense in v0:** the sequencer signs blob submissions to Celestia, but the runtime independently re-validates every transaction in every blob. A compromised sequencer key cannot bypass attestation signature checks, fee accounting, or any other state-transition rule.

**What it can do:** replace the canonical block content with adversary-chosen valid transactions. This is censorship plus reordering, not arbitrary state-write capability.

**Future hardening:** multi-sequencer ([#82](https://github.com/ligate-io/ligate-chain/issues/82)); slashing on equivocation in v1+.

---

## 3. Wire-format and runtime adversaries

Attacks targeting the chain's encoding contract or runtime composition.

### 3.1 Borsh deserialization adversary

**Attack:** craft input that causes the Borsh decoder to panic, allocate unboundedly, or accept malformed records.

**Defense:** every variable-length field on a public type is wrapped in `SafeVec<T, MAX>` or `SafeString` with a fixed cap. The cap is encoded in the type signature, so the decoder rejects oversize inputs at the type level. Public protocol enums are `#[non_exhaustive]` so future variants do not break downstream `match` arms.

**Where:** `MAX_ATTESTOR_SET_MEMBERS = 64`, `MAX_ATTESTATION_SIGNATURES = 64`, `MAX_ATTESTOR_SIGNATURE_BYTES = 96` in `crates/modules/attestation/src/lib.rs`. `#[non_exhaustive]` shipped via [PR #103](https://github.com/ligate-io/ligate-chain/pull/103).

**Future hardening:** dedicated fuzz targets for Borsh decoders, signatures, and Bech32m ([#119](https://github.com/ligate-io/ligate-chain/issues/119)).

### 3.2 Wire-format drift

**Attack:** silently change an on-chain type's encoding so two operators interpret the same bytes differently and diverge in state.

**Defense:** 13 Borsh wire-format snapshot tests in `crates/modules/attestation/tests/borsh_snapshot.rs`. Every public protocol type (`SchemaId`, `AttestorSetId`, `PayloadHash`, `AttestationId`, `Schema`, `AttestorSet`, `Attestation`, `SignedAttestationPayload`, all `CallMessage` variants) has a deterministic byte snapshot. A PR that changes the encoding fires red on CI.

**Where:** `crates/modules/attestation/tests/borsh_snapshot.rs`; shipped via [PR #108](https://github.com/ligate-io/ligate-chain/pull/108) closing [#45](https://github.com/ligate-io/ligate-chain/issues/45). Hard-fork policy in [`upgrades.md` §What counts as a hard fork](upgrades.md#what-counts-as-a-hard-fork).

### 3.3 Runtime composition drift

**Attack:** two operators run subtly different runtime compositions (different module set, different `Config` shapes) and produce divergent state.

**Defense:** `CHAIN_HASH` constant derived deterministically from the runtime's universal-wallet schema at build time. Every signed transaction is domain-separated by this hash. Two operators who do not agree on the runtime composition produce signatures that fail verification on each other's nodes.

**Where:** `crates/stf/build.rs`; consumed by `crates/stf/src/runtime_capabilities.rs`. Documented in [`upgrades.md` §The CHAIN_HASH guard](upgrades.md#the-chain_hash-guard).

### 3.4 Address-format spoofing

**Attack:** produce a valid-looking address that decodes to bytes belonging to a different account.

**Defense:** Bech32m encoding with HRP-checked decode. The HRP (`lig`, `lpk`, `lsc`, `las`, `lph`) is part of the encoded data; a string with the wrong HRP fails to decode. Bech32m's BCH error-correction also rejects single-character corruption.

**Where:** `impl_hash32_type!` in `sov_modules_api`, used in `crates/modules/attestation/src/lib.rs` for each newtype. Documented in [`addresses-and-signing.md`](addresses-and-signing.md).

---

## 4. Cryptographic primitive failure

### 4.1 Ed25519 break

**Attack:** signature forgery against the chain's primary signing scheme.

**Defense in v0:** none specific. The chain inherits whatever post-quantum stance the underlying primitives carry.

**Posture:** ed25519 is the only signature scheme in v0. A successful attack against ed25519 would compromise every attestation, every transaction, and every multi-sig set. There is no chain-level defense against a primitive break; defense is by primitive choice and migration readiness if a break is foreseen.

**Future hardening:** quantum-resistant variants would require a coordinated chain-id bump and a re-key ceremony for every account. Not in v0 through v3 scope.

### 4.2 SHA-256 collision

**Attack:** find two inputs that hash to the same `payload_hash` or `schema_id`, breaking the write-once invariant.

**Defense:** none specific to the chain. SHA-256 collision resistance is the underlying assumption; a break is catastrophic for any chain that uses SHA-256 as a content-address.

**Same answer as 4.1:** primitive choice plus migration readiness, no chain-level mitigation.

---

## 5. Operational adversaries

Attacks against the development and deployment pipeline.

### 5.1 Direct push to main

**Attack:** push malicious code directly to `main` without review.

**Defense:** branch protection ruleset on `main` (id `15619891`) blocks force-pushes and direct pushes. PRs require 1 approving review, dismissed on push, with thread resolution required. The required check is the `CI pass` summary job.

**Caveat:** the founder is in the bypass list to allow solo merges via `gh pr merge --admin`. This drops to standard external-review when a second engineer joins. Until then the bypass is the operational SPOF.

**Where:** ruleset documented in `~/Desktop/ligate-docs/chain/STATE.md` (private), enforced by GitHub.

### 5.2 Supply-chain compromise

**Attack:** introduce a malicious or vulnerable dependency.

**Defense:** `cargo audit` runs on every PR (including docs-only) and fires red on any new RUSTSEC advisory. Ignored advisories live in `audit.toml` with documented rationale (each entry is a transitive dep pinned by the Sovereign SDK). New advisories trigger an immediate add-to-ignore-with-rationale or a pin bump.

**Where:** [`.github/workflows/ci.yml`](../../.github/workflows/ci.yml), [`audit.toml`](../../audit.toml). Pin-review cadence tracked in [#115](https://github.com/ligate-io/ligate-chain/issues/115).

### 5.3 CI-side state exfiltration

**Attack:** introduce a workflow that exfiltrates secrets, write tokens, or build artifacts.

**Defense in v0:** GitHub workflow permissions are scoped per-job; the project does not currently set repository-wide permissive defaults. Dependabot is sandboxed to its own permission boundary. New workflows require human review (any `.github/workflows/**` change triggers full code CI per the path filter).

**Future hardening:** [`CODEOWNERS`](https://github.com/ligate-io/ligate-chain/issues/113) for per-path review routing on `.github/workflows/**`.

---

## 6. What is deliberately out of scope for v0

Tracked separately. Not a v0 acceptance gap; a roadmap item.

| Risk | Tracked | Lands |
|---|---|---|
| Force-include path | [#81](https://github.com/ligate-io/ligate-chain/issues/81) | v1 |
| Multi-sequencer / leader rotation | [#82](https://github.com/ligate-io/ligate-chain/issues/82) | v1 |
| Disputes (attestor slashing) | [#51](https://github.com/ligate-io/ligate-chain/issues/51) | v1 |
| Staking module | [#50](https://github.com/ligate-io/ligate-chain/issues/50) | v1 |
| Stake-weighted prover marketplace | [#85](https://github.com/ligate-io/ligate-chain/issues/85) | v2 |
| Cryptographic inference verification | schema-level via external proof hashes | v2 |
| EVM-shaped tx auth | [#72](https://github.com/ligate-io/ligate-chain/issues/72) | post-mainnet option |
| Cross-chain bridges | [#73](https://github.com/ligate-io/ligate-chain/issues/73) Hyperlane | v2 |
| PoUA cost-to-attack ($\kappa$) | [#126](https://github.com/ligate-io/ligate-chain/issues/126) umbrella | v3 mainnet |
| Quantum-resistant primitives | inherits SDK posture | v4+ |
| Privacy-preserving payloads (zk inference) | schema-level commitments | v2+ |

The PoUA-specific threat model (capital adversary, reputation adversary, compound capital-plus-grinding adversary) is in [the working paper §5](https://github.com/ligate-io/ligate-research/blob/main/papers/poua/poua.pdf). It applies to v3 mainnet, not v0.

---

## 7. Protection inventory

A flat lookup of every v0 protection mechanism with its file location.

| Mechanism | Where | Status |
|---|---|---|
| Write-once `(schema_id, payload_hash)` | `crates/modules/attestation/src/lib.rs` | shipped |
| `fee_routing_bps` cap at 5000 | `crates/modules/attestation/src/lib.rs` | shipped |
| AttestorSet `M-of-N` threshold | `crates/modules/attestation/src/lib.rs` | shipped |
| Per-attestation fee + registration fees | `crates/modules/attestation/src/lib.rs` | shipped |
| `SafeVec<T, MAX>` + `SafeString` caps | `crates/modules/attestation/src/lib.rs` | shipped |
| `#[non_exhaustive]` on public enums | `crates/modules/attestation/src/lib.rs` | shipped via [#103](https://github.com/ligate-io/ligate-chain/pull/103) |
| Borsh wire-format snapshot tests | `crates/modules/attestation/tests/borsh_snapshot.rs` | shipped via [#108](https://github.com/ligate-io/ligate-chain/pull/108) |
| `CHAIN_HASH` runtime-composition guard | `crates/stf/build.rs` + `runtime_capabilities.rs` | shipped |
| `CHAIN_ID = 4242` tx domain separation | `constants.toml` | shipped |
| Bech32m HRP-checked addresses | `impl_hash32_type!` macro | shipped |
| `cargo audit` on every PR | `.github/workflows/ci.yml`, `audit.toml` | shipped |
| Branch protection on `main` | GitHub ruleset `15619891` | shipped |
| DA-agnostic crate isolation | `scripts/check-da-isolation.sh` + CI | shipped via [#118](https://github.com/ligate-io/ligate-chain/pull/118) |
| Fuzz targets (Borsh, signatures, Bech32m) | follow-up | tracked: [#119](https://github.com/ligate-io/ligate-chain/issues/119) |
| `cargo-deny` for license / dup / source policy | follow-up | tracked: [#120](https://github.com/ligate-io/ligate-chain/issues/120) |
| `CODEOWNERS` per-path review routing | follow-up | tracked: [#113](https://github.com/ligate-io/ligate-chain/issues/113) |
| Negative-path test audit | follow-up | tracked: [#124](https://github.com/ligate-io/ligate-chain/issues/124) |
| End-to-end smoke test | shipped via [#145](https://github.com/ligate-io/ligate-chain/pull/145) | closes [#123](https://github.com/ligate-io/ligate-chain/issues/123) |

---

## 8. Incident response

The disclosure policy lives in [`SECURITY.md`](../../SECURITY.md). Summary:

- **Primary channel:** GitHub Private Security Advisories at `https://github.com/ligate-io/ligate-chain/security/advisories/new`
- **Fallback:** email `hello@ligate.io` with `[SECURITY]` subject prefix
- **Acknowledgement SLA:** 2 business days
- **Coordinated disclosure:** 90 days default
- **Bounty:** none formally pre-mainnet; ad-hoc rewards for high-impact reports

Public GitHub issues for suspected vulnerabilities are not appropriate; a public issue makes the bug exploitable the moment it lands.

---

## Cross-references

- [`attestation-v0.md`](attestation-v0.md): protocol specification (entities, state, fees, invariants)
- [`addresses-and-signing.md`](addresses-and-signing.md): Bech32m HRP map, signature scheme rationale
- [`upgrades.md`](upgrades.md): soft-fork vs hard-fork policy, `CHAIN_HASH` guard
- [`da-layers.md`](da-layers.md): DA-agnostic architecture and adapter playbook (DA-layer trust posture)
- [`rest-api.md`](rest-api.md): full REST endpoint reference
- [`SECURITY.md`](../../SECURITY.md): disclosure policy
- [`devnet/README.md`](../../devnet/README.md): local boot recipe
- [`docs/development/devnet.md`](../development/devnet.md): operator runbook

## Related issues

- [#121](https://github.com/ligate-io/ligate-chain/issues/121): closed by this document
- [#119](https://github.com/ligate-io/ligate-chain/issues/119): Fuzz targets
- [#120](https://github.com/ligate-io/ligate-chain/issues/120): cargo-deny policy
- [#113](https://github.com/ligate-io/ligate-chain/issues/113): CODEOWNERS
- [#124](https://github.com/ligate-io/ligate-chain/issues/124): negative-path test audit
