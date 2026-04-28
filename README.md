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

- [`crates/modules/attestation`](crates/modules/attestation): the attestation protocol module. Data shapes, state layout, call handlers, signature validation, and fee routing through `sov-bank`.
- [`crates/stf`](crates/stf): the runtime crate that composes the chain's modules (`bank`, `accounts`, `sequencer_registry`, `attester_incentives`, `prover_incentives`, `operator_incentives`, `attestation`) into a state-transition function. `CHAIN_HASH` is derived from the runtime schema at build time so genesis can't drift from code.
- [`crates/stf-declaration`](crates/stf-declaration): the runtime declaration consumed by the SDK macros.
- [`crates/rollup`](crates/rollup): the node binary (`ligate-node`). Wires the STF to the Celestia DA adapter and exposes the RPC surface.
- [`crates/rollup/provers/risc0`](crates/rollup/provers/risc0): the Risc0 inner zkVM prover, including the Celestia guest binary that runs the full STF inside the zkVM.
- [`crates/client-rs`](crates/client-rs): the Rust client SDK for applications talking to the chain. Typed builders and ed25519 helpers on the new SDK.

Devnet config (genesis files, Celestia and rollup TOMLs) lives in [`devnet/`](devnet) and is checked in so anyone can boot a local devnet against Celestia mocha-testnet.

Protocol docs:

- [`docs/protocol/attestation-v0.md`](docs/protocol/attestation-v0.md): attestation protocol specification v0.
- [`docs/protocol/addresses-and-signing.md`](docs/protocol/addresses-and-signing.md): how Ligate addresses (`lig1…`) work, why MetaMask doesn't sign for the chain today, and what changes when EVM auth ([#72](https://github.com/ligate-io/ligate-chain/issues/72)) lands.

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

**Pre-devnet.** No public chain is running yet, but the protocol works end-to-end against a local Celestia mocha-testnet. As of Phase A:

- Attestation module: data types, state layout, call handlers, ed25519 signature validation, replay protection, fee routing.
- Runtime: `ligate-stf` composes the curated module set on the upgraded Sovereign SDK.
- DA: Celestia adapter wired for block submission and ingestion.
- ZK: Risc0 inner zkVM with a Celestia guest binary that runs the full STF.
- Fees: real `$LGT` transfers through `sov-bank`, with up to 50% of attestation fees routable to the schema owner.
- Genesis: checked-in devnet config (bank, attestation, attester incentives, prover incentives, sequencer registry, operator incentives).
- Addresses: `lig1` accounts, `lpk1` pubkeys, `lsc1` schema IDs, `las1` attestor set IDs, `lph1` payload hashes (Bech32m).

Open work items toward public devnet: explorer, faucet, hosted RPC endpoint, EVM authenticator ([#72](https://github.com/ligate-io/ligate-chain/issues/72)), and the v1 staking and disputes modules. Current tracked work lives in the [GitHub issues](https://github.com/ligate-io/ligate-chain/issues).

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

- Apache License, Version 2.0 ([`LICENSE-APACHE`](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT License ([`LICENSE-MIT`](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this repository by you, as defined in the Apache-2.0 license, shall be dual-licensed as above, without any additional terms or conditions.
