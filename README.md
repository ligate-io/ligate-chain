# Ligate Chain

Ligate Chain: the on-chain attestation protocol. A Sovereign SDK rollup on Celestia.

## What is this repo

This repository holds the **Ligate Chain protocol** and its first-party SDKs. Its one core product in v0 is a generic **attestation protocol**: anyone can register an attestor set and a schema, then submit cryptographically signed records ("attestations") under it that anyone can verify later. The chain stores only hashes and signatures, never plaintext payloads.

Product-specific code (Themisra, Iris, Kleidon) lives in separate repositories. Those products are the first consumers of this protocol, not part of it. They register their own schemas and attestor sets and submit attestations through the same public interface any other builder would use.

Who this repo is for:

- **App builders** who want to attach a verifiable, on-chain receipt to something that happened off-chain (model inferences, purchases, tickets, state transitions). The protocol is deliberately narrow and generic.
- **Node operators** running or planning to run a Ligate Chain node.
- **Protocol contributors** working on the rollup, the module, or the SDK.

The canonical specification is in [`docs/protocol/attestation-v0.md`](docs/protocol/attestation-v0.md). Read it before opening a non-trivial PR against `crates/modules/attestation`.

## What Ligate Chain is, and isn't

Ligate Chain is a **specialized app-chain**, not a general-purpose smart-contract platform. The closer comparisons are Celestia, Hyperliquid, dYdX v4, or Cosmos app-chains: each chain has a narrow remit and is shaped around it. Our remit is on-chain attestation infrastructure for AI claims.

**It is:**

- A sovereign rollup on Celestia DA. Own state, own token ($LGT), own sequencer, own protocol logic.
- A curated module set. Today: `attestation`. Planned: `tokens` ([#47](https://github.com/ligate-io/ligate-chain/issues/47)) and `nft` ([#48](https://github.com/ligate-io/ligate-chain/issues/48)) in v1, `payments`, `agents`, `identity`, `disputes` later. Each module is a designed product surface, not a sandbox.
- Permissionless at the **schema** level: anyone registers a schema on the attestation module and submits attestations under it. No gatekeeper.

**It isn't:**

- An EVM chain in v0 / v1 / v2 / v3. There is no Solidity, no contract deployment, no ERC-20 deploy via tx in any of those phases. Tokens and NFTs come through the curated `tokens` and `nft` modules. EVM compatibility is tracked as a long-horizon v4 option ([#52](https://github.com/ligate-io/ligate-chain/issues/52)) we will revisit only after the attestation focus has paid off in shipped, in-production users; until then we don't dilute the chain identity for it.
- An L1 in the Bitcoin / Ethereum sense. We use Celestia for data availability rather than building our own DA layer. We are sovereign in the rollup sense (no settlement to Ethereum, our own validator set), but DA is outsourced.
- An L2 in the Optimism / Arbitrum sense. We do not settle to Ethereum.
- A general DeFi platform. The chain is shaped for attestation primitives plus a few sister modules. Lending, AMMs, perps, and similar live on chains designed for them.

If you want to build a generalised dApp, this is the wrong chain. If you want verifiable on-chain receipts for AI activity at scale, this is the chain we are building for that.

## Workspace layout

Cargo workspace (resolver 2). Members:

- [`crates/modules/attestation`](crates/modules/attestation): the attestation protocol module. Data shapes, state layout, call handlers, and signature validation. This is the only crate with real protocol code today.
- [`crates/rollup`](crates/rollup): the node binary (`ligate-node`). Scaffold only. Full Sovereign SDK wiring (runtime, DA layer adapter, genesis config, RPC server) is in progress.
- [`crates/client-rs`](crates/client-rs): the Rust client SDK for applications talking to the chain. Scaffold only. Surface lands alongside the node.

Protocol docs:

- [`docs/protocol/attestation-v0.md`](docs/protocol/attestation-v0.md): attestation protocol specification v0.

## Build and test

Rust toolchain is pinned via [`rust-toolchain.toml`](rust-toolchain.toml) to 1.93.0. Any modern `rustup` will install it automatically when you enter the repo.

### System dependencies

The SDK pulls in `librocksdb-sys`, which needs `libclang` at build time:

- **Ubuntu / Debian**: `sudo apt install libclang-dev clang`
- **macOS**: ships with the Command Line Tools (`xcode-select --install`); you also need to set the dyld path so the `librocksdb-sys` build script can find the dylib at link time. Add to `~/.zshrc` or `~/.bashrc`:
  ```bash
  export DYLD_FALLBACK_LIBRARY_PATH=/Library/Developer/CommandLineTools/usr/lib
  export LIBCLANG_PATH=/Library/Developer/CommandLineTools/usr/lib
  ```
  Or set them per-invocation: `DYLD_FALLBACK_LIBRARY_PATH=/Library/Developer/CommandLineTools/usr/lib LIBCLANG_PATH=/Library/Developer/CommandLineTools/usr/lib cargo test ...`.

### CI gates

```bash
# Compile check across the workspace
cargo check --workspace

# Run every test in every crate
cargo test --workspace

# Formatting (CI runs this with --check)
cargo fmt --all -- --check

# Clippy (CI treats warnings as errors)
cargo clippy --workspace --all-targets -- -D warnings

# Docs (CI treats rustdoc warnings as errors)
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items

# Security advisories (install once with: cargo install cargo-audit)
cargo audit
```

Ignored advisories and their rationales are documented in [`audit.toml`](audit.toml). All current ignores are transitive dependencies pinned by the Sovereign SDK; they clear automatically on SDK upgrades.

## Protocol at a glance

The v0 protocol has three entities. Full detail in [the spec](docs/protocol/attestation-v0.md).

### AttestorSet

A quorum of ed25519 signers with an M-of-N threshold. Identified by `SHA-256(sorted_members || threshold)`. Immutable once registered; to "rotate" signers, register a new set and point a new schema version at it.

### Schema

An application's attestation shape: `owner`, `name`, `version`, the `AttestorSetId` that must sign under it, and optional fee routing (cap 50% to the schema owner, remainder to treasury). Identified by `SHA-256(owner || name || version)`, so two different owners can safely reuse the same human-readable name. Registration is permissionless and gated only by the registration fee in `$LGT`.

### Attestation

The on-chain record: `(schema_id, payload_hash, submitter, timestamp, signatures)`, keyed by `(schema_id, payload_hash)` and write-once. The chain verifies the supplied signatures against the schema's attestor set at submit time; later reads are cheap RPC queries against a `StateMap`.

## Development status

**Pre-devnet.** No public chain is running yet. The attestation module has its data types, state layout, call handlers, and signature validation wired up and under test (see `crates/modules/attestation/src/lib.rs`); the rollup binary and client SDK are scaffolds. Fee charging, genesis seeding, RPC endpoints, and block-header timestamp plumbing are open work items.

Current tracked work lives in the [GitHub issues](https://github.com/ligate-io/ligate-chain/issues). If you are considering a contribution, start there.

v0 scope is the attestation protocol only. Identity, disputes/slashing, payments, and the agent registry are explicit v1 and v2 non-goals documented in the spec.

## Contributing

Required reading before a non-trivial PR:

1. [`docs/protocol/attestation-v0.md`](docs/protocol/attestation-v0.md) for the protocol itself.
2. [`.github/workflows/ci.yml`](.github/workflows/ci.yml) for the CI jobs your branch has to pass.

CI runs six jobs and all must pass: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo check`, `cargo test`, `cargo doc` (with `RUSTDOCFLAGS=-D warnings`), and `cargo audit`. Please run them locally before pushing; the full list is in the "Build and test" section above.

Commit style: one-line subject (imperative mood, under ~72 chars), blank line, short body explaining the why. Keep changes focused; open separate PRs for independent concerns.

Issues, questions, and bug reports: [github.com/ligate-io/ligate-chain/issues](https://github.com/ligate-io/ligate-chain/issues).

## License

Licensed under either of:

- Apache License, Version 2.0
- MIT License

at your option. See the workspace manifest ([`Cargo.toml`](Cargo.toml)) for the canonical license declaration.
