# Genesis determinism

`ligate-genesis-tool generate` reads a template directory + a substitutions TOML, writes a new genesis bundle. Two operators on different machines (different Rust versions, different OSes, different filesystems) running the tool against the same inputs must produce a **byte-identical** output. Otherwise multi-operator validation is silently broken: two operators compute different `CHAIN_HASH` values from the same input and never realize.

CI verifies this via `.github/workflows/genesis-determinism.yml`: cross-OS matrix (Linux + macOS) running the tool with a fixture substitution, sha256-comparing each output file. The job fires on any PR that touches `crates/genesis-tool/`, `devnet-1/genesis/`, or the workflow itself.

---

## Sources of non-determinism we mitigate (or proved absent)

| Source | Affects | Mitigation in tool |
|---|---|---|
| HashMap iteration order | JSON object field order | Tool uses `serde_json::to_writer_pretty` against `BTreeMap`-backed structs (sorted by key) wherever the input is structurally a map. The 8 genesis-config structs are `#[derive(Serialize)]` over named fields, which serde emits in declaration order — deterministic. |
| Timestamp injection | Any field carrying creation-time | Tool does NOT timestamp the output; every output field comes from the template or substitutions input. |
| Locale-dependent JSON formatting | Number / decimal formatting | `serde_json` emits ASCII-only JSON; locale is irrelevant. |
| Floating-point representation | Currency fields | All amounts are `u128`-backed decimal strings (per RFC 0002); no f64 anywhere. |
| Filesystem-order directory traversal | Order of files written | Tool iterates a fixed `GENESIS_FILES` array, not a directory listing. Deterministic across filesystems. |
| Trailing newline / line-ending | EOF byte sequences | `to_writer_pretty` appends a single `\n`; no platform-dependent CRLF. |
| Hash of derived state | `CHAIN_HASH` build-time derive | `CHAIN_HASH` is computed via `Schema`'s deterministic-hasher path with pinned spec parameters (`MockDaSpec + MockZkvm + MultiAddressEvm`) regardless of the binary's runtime DA/ZkVM choice. See [`crates/stf/build.rs`](../../crates/stf/build.rs). |

Each row above is what the *tool* mitigates. The cross-OS CI test catches anything we didn't think of — that's its job.

---

## When the CI test fires

A divergence between `linux-amd64` and `darwin-arm64` output means a new non-determinism source slipped in. Common culprits:

- **Added a `HashMap<K, V>` to a serialised struct.** Swap for `BTreeMap` or sort before serialise. Serde's `Serialize` uses HashMap iteration order which differs across platforms.
- **Added an integer field formatted with `Display`** where the integer comes from a chrono-time / random / process-id source. Make the field input-derived only.
- **Introduced a new `serde_json::Value` round-trip** that lost field ordering. Use typed structs instead.
- **Added a custom `Hasher` somewhere that depends on platform-default random state.** Wire a deterministic hasher (e.g. `seahash` with a fixed seed) explicitly.

The CI job's failure message points at the diverged files; the uploaded `genesis-out-*` artifacts let you `diff -r` to find the exact field.

---

## How to reproduce a CI failure locally

```sh
# Build the tool
cargo build --release -p ligate-genesis-tool

# Generate twice with the same inputs; sha256 each file
mkdir -p /tmp/run-a /tmp/run-b
./target/release/ligate-genesis-tool generate \
    --template devnet-1/genesis \
    --substitutions ops/genesis-determinism/substitutions.toml \
    --output /tmp/run-a

./target/release/ligate-genesis-tool generate \
    --template devnet-1/genesis \
    --substitutions ops/genesis-determinism/substitutions.toml \
    --output /tmp/run-b

# Compare
find /tmp/run-a -type f -exec shasum -a 256 {} \; | sort > /tmp/a.sha
find /tmp/run-b -type f -exec shasum -a 256 {} \; | sort > /tmp/b.sha
diff -u /tmp/a.sha /tmp/b.sha
```

Same-machine non-determinism (different output between two consecutive runs on one host) catches HashMap-iteration-order regressions before they even reach CI.

For cross-OS reproduction without GitHub Actions: a Linux VM (via Multipass / Lima / a Docker container with the matching toolchain) running the above and uploading its `/tmp/a.sha` for comparison against the same on macOS.

---

## Trust model

This is operational determinism, not cryptographic determinism. The tool's output is reproducible because the *implementation* doesn't introduce variation, not because the output is signed or attested. If a non-determinism source slips in despite our checks, the failure mode is "two operators see different state" — caught downstream by the chain's `CHAIN_HASH` mismatch at first transaction signing. The CI test moves that catch from "first signed tx fails on a partner-operator's node, hours later" to "PR doesn't merge."

---

## Out of scope

- **Cross-Rust-version determinism.** Pinned to 1.93.0 via `rust-toolchain.toml`. A future Rust upgrade is its own deliberate PR with the same determinism test re-run.
- **Cross-architecture (CPU) endianness.** All chain encodings use little-endian Borsh; no platform-conditional code paths in genesis-tool today.
- **Tamper-evidence of the output.** Operator signs the genesis bundle separately if they want a tamper-evident artifact (covered by [`chain-id-bump.md`](runbooks/chain-id-bump.md)'s pre-bump checklist, "Snapshot any state that should carry over").

---

## Cross-references

- [Issue #278](https://github.com/ligate-io/ligate-chain/issues/278) — this work closes it
- [`crates/genesis-tool/src/main.rs`](../../crates/genesis-tool/src/main.rs) — the tool
- [`.github/workflows/genesis-determinism.yml`](../../.github/workflows/genesis-determinism.yml) — the CI matrix
- [`ops/genesis-determinism/substitutions.toml`](../../ops/genesis-determinism/substitutions.toml) — the fixture
- [`crates/stf/build.rs`](../../crates/stf/build.rs) — `CHAIN_HASH` derivation (related determinism layer)
- [`upgrades.md`](../protocol/upgrades.md#the-chain_hash-guard) — the `CHAIN_HASH` guard story
