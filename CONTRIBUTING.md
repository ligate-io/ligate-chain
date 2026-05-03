# Contributing to Ligate Chain

Thanks for the interest. This document covers everything from cloning the repo to merging a PR.

The short version: read [`README.md`](README.md) for what the chain is, [`docs/protocol/attestation-v0.md`](docs/protocol/attestation-v0.md) for how the protocol actually works, run `cargo test --workspace` to verify your environment, then open a focused PR with a clear test plan. The longer version follows.

## Table of contents

- [Code of Conduct](#code-of-conduct)
- [Local development setup](#local-development-setup)
- [Project layout](#project-layout)
- [Running tests locally](#running-tests-locally)
- [Code style](#code-style)
- [Commit conventions](#commit-conventions)
- [Pull request conventions](#pull-request-conventions)
- [Required reading for protocol PRs](#required-reading-for-protocol-prs)
- [CI gates](#ci-gates)
- [Filing issues](#filing-issues)
- [Security disclosures](#security-disclosures)

## Code of Conduct

This project follows the [Contributor Covenant 2.1](CODE_OF_CONDUCT.md). By participating, you agree to abide by it. Reports go to `hello@ligate.io` with subject prefix `[CONDUCT]`.

## Local development setup

### Toolchain

The Rust toolchain is pinned in [`rust-toolchain.toml`](rust-toolchain.toml) (currently `1.93.0`). Any modern `rustup` will install it automatically when you `cd` into the repo or run `cargo build` for the first time. Don't override the toolchain unless you're explicitly testing a different version.

### System dependencies

The chain pulls in `librocksdb-sys`, which needs `libclang` at build time:

**Ubuntu / Debian**

```bash
sudo apt install -y libclang-dev clang
```

**macOS**

The Command Line Tools ship with what's needed:

```bash
xcode-select --install
```

Then export the dyld and libclang paths so `librocksdb-sys`'s build script can find the dylib at link time. Add to `~/.zshrc` or `~/.bashrc`:

```bash
export DYLD_FALLBACK_LIBRARY_PATH=/Library/Developer/CommandLineTools/usr/lib
export LIBCLANG_PATH=/Library/Developer/CommandLineTools/usr/lib
```

Or set them per-invocation:

```bash
DYLD_FALLBACK_LIBRARY_PATH=/Library/Developer/CommandLineTools/usr/lib \
LIBCLANG_PATH=/Library/Developer/CommandLineTools/usr/lib \
cargo test --workspace
```

### macOS-only Risc0 workaround

The Risc0 prover crate (`crates/rollup/provers/risc0`) builds Metal kernels on macOS. If your Command Line Tools install is missing the Metal SDK (i.e. you don't have Xcode proper), set:

```bash
export RISC0_SKIP_BUILD_KERNELS=1
```

This is a per-machine workaround. **Do not** set it in CI — Linux runners build the CPU kernels cleanly, and skipping there produces unresolved-symbol errors at link time. CI also sets `SKIP_GUEST_BUILD=1` to skip the heavy ~10-minute zkVM guest compilation; that one's fine to mirror locally.

### First build

```bash
git clone https://github.com/ligate-io/ligate-chain
cd ligate-chain
cargo build --workspace
```

Cold build: ~3-5 minutes on a recent dev machine. Warm rebuild: seconds.

## Project layout

The `README.md` `## Workspace layout` section is the canonical reference. Quick map:

- `crates/modules/attestation` — the attestation protocol module (state, call handlers, REST queries)
- `crates/stf`, `crates/stf-declaration` — runtime composition (the chain's modules wired into a state-transition function)
- `crates/rollup` — the `ligate-node` binary
- `crates/rollup/provers/risc0` — Risc0 inner zkVM prover, with a Celestia guest sub-workspace
- `crates/client-rs` — Rust client SDK
- `devnet/` — checked-in genesis JSONs + Mock/Celestia rollup TOMLs
- `docs/protocol/` — protocol specifications
- `docs/development/` — operator runbooks

## Running tests locally

```bash
# Workspace-wide library + integration + doc tests
cargo test --workspace

# Just one crate (faster iteration)
cargo test -p attestation --all-features

# A single test by name
cargo test -p attestation query_get_schema_returns_registered_schema

# Skip the heavy zkVM guest build (matches CI)
SKIP_GUEST_BUILD=1 cargo test --workspace
```

The end-to-end smoke test in [`crates/rollup/tests/e2e_smoke.rs`](crates/rollup/tests/e2e_smoke.rs) exercises the production runtime + auto-mounted REST surface against `TestRunner` with a genesis-seeded attestor set and schema. It runs automatically as part of `cargo test --workspace`. To run it in isolation:

```bash
cargo test -p ligate-rollup --test e2e_smoke
```

If you're touching a state map, an attestor signature path, or the genesis loader, run the full suite. There are 29+ integration tests that exercise the cross-module invariants (sequencer-registry locking, attester/prover bonds, fee routing through `sov-bank`).

## Code style

### rustfmt

`rustfmt` config is in [`rustfmt.toml`](rustfmt.toml). CI runs:

```bash
cargo fmt --all -- --check
```

If your local rustfmt version differs from CI's pinned `1.93.0`, the check might disagree. Easy fix: re-run `rustup update`, or run `cargo +1.93.0 fmt --all` explicitly.

### clippy

CI treats every clippy warning as an error:

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Don't `#[allow(...)]` to silence a clippy warning unless you've left a comment explaining *why* the lint is wrong for your case.

### rustdoc

Public items must have a doc comment. CI builds docs with `RUSTDOCFLAGS=-D warnings`:

```bash
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items
```

Comment density expectation for public protocol types (`Schema`, `AttestorSet`, `Attestation`, `CallMessage`, etc.): every field needs a doc comment, every variant needs a doc comment, every method needs a doc comment with a one-line summary. These are the surface third-party builders consume; treat them like API docs, not internal notes.

For module-internal helpers, doc comments are encouraged but not enforced.

## Commit conventions

- **One-line subject.** Imperative mood, ideally under 72 characters. Examples from the repo: `Add LICENSE-APACHE and LICENSE-MIT files to match workspace declaration`, `Skip code-checking jobs on docs-only PRs via paths-filter + CI summary check`.
- **Optional body.** Blank line then a paragraph explaining *why* the change is shaped this way, not what it does (the diff covers what).
- **No `Co-Authored-By` trailer.** Repo-wide convention.
- **No `Signed-off-by`.** We don't run a DCO.
- **Reference issues** in the subject or body when relevant: `(refs #21)`, `(closes #76)`.
- **One concern per commit.** If you're adding a feature *and* fixing an unrelated bug, that's two commits.

## Pull request conventions

- **One concern per PR.** Reviewers should be able to summarise your PR in one sentence. If they can't, split it.
- **Test plan in the description.** Use the `.github/PULL_REQUEST_TEMPLATE.md` checkboxes — it forces you to think through what you've actually verified.
- **CI must be green.** The required check is `CI pass` (a summary that aggregates `fmt`, `clippy`, `check`, `test`, `doc`, `audit`). Docs-only PRs skip the heavy gates via `paths-filter`; that's intentional.
- **Reviews.** One approving review required (currently bypassed for the founder; this changes when a second engineer joins). For external contributors, expect a review turnaround of 1-3 business days.
- **Rebasing vs. merging.** No hard preference — `merge`, `squash`, and `rebase` are all allowed. Squash is the default for tidy single-commit changes.

## Required reading for protocol PRs

If your PR touches `crates/modules/attestation`, the runtime composition in `crates/stf`, or any state shape that ends up in `devnet/genesis/*.json`, read these first:

1. [`docs/protocol/attestation-v0.md`](docs/protocol/attestation-v0.md) — the protocol specification (entities, state layout, fees, invariants)
2. [`docs/protocol/addresses-and-signing.md`](docs/protocol/addresses-and-signing.md) — why the chain signs ed25519 natively, what `lig1...` addresses are, and what changes when EVM auth lands (#72)

Wire-format-affecting changes (Borsh layout shifts, `MAX_*` cap bumps, new `CallMessage` variants) need a migration story before they merge. Open a discussion or draft PR if you're unsure whether your change qualifies.

## CI gates

The required gate is **`CI pass`**. It's a summary job that watches the underlying jobs and accepts:

| Job | Purpose | Skipped on docs-only PRs? |
|---|---|---|
| `cargo fmt` | Format check (`-- --check`) | yes |
| `cargo clippy` | Lints (`-D warnings`) | yes |
| `cargo check` | Compile check (`--workspace --all-targets`) | yes |
| `cargo test` | Workspace lib + integration + doctests | yes |
| `cargo doc` | rustdoc with `-D warnings` | yes |
| `cargo audit` | RUSTSEC advisories | **no** — runs on every PR |
| `cargo deny` | License / duplicate / source policy | yes |

Why `cargo audit` ignores the path filter: new advisories appear independent of your diff. We want every PR to be a checkpoint for security signal.

`cargo deny` enforces three policies on top of `cargo audit`:

- **Licenses**: every transitive dep must resolve to a license in the allow list. New deps with copyleft or non-OSI licenses fail CI.
- **Sources**: git deps may only come from the explicit allow list (currently the Sovereign SDK + its `rockbound` storage repo). Stops a stray `git = "..."` URL from sneaking in.
- **Bans**: denies `native-tls`, `openssl`, and `openssl-sys`. We're Rustls-only; a transitive that flips us back to system OpenSSL would surprise the deploy story.

Local equivalent (run before pushing):

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo check --workspace --all-targets
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items
cargo audit
cargo deny check  # brew install cargo-deny  (or `cargo install cargo-deny --locked`)
```

Ignored advisories live in [`audit.toml`](audit.toml) — every entry is a transitive dep pinned by the Sovereign SDK. They clear automatically on SDK upgrades.

Policy rules and per-crate exceptions live in [`deny.toml`](deny.toml). The Sovereign SDK ships under a custom commercial license (the SPCL); its crates are clarified to that license-ref. LGPL transitives (`malachite*`, `downloader`, `r-efi`) are pulled by risc0. Rationale for each exception is inline in the file.

## Fuzzing

The protocol's untrusted-byte decoders (Borsh, Bech32m, REST path parsers) get nightly coverage via `cargo-fuzz`. Targets live in [`crates/modules/attestation/fuzz/`](crates/modules/attestation/fuzz/) and run on the [`Fuzz`](.github/workflows/fuzz.yml) workflow each night against a ~10 min total time budget across all targets. Cron-only by design: every-PR fuzzing without a checked-in corpus burns runner minutes for marginal coverage gain.

Targets cover Borsh deserialization of `CallMessage`, `Schema`, `AttestorSet`, `Attestation`, `SignedAttestationPayload`, plus Bech32m decode for the four typed IDs (`SchemaId`, `AttestorSetId`, `PayloadHash`, `PubKey`) and `AttestationId` colon-separated round-trips. Tracking issue: [#119](https://github.com/ligate-io/ligate-chain/issues/119).

Run locally:

```bash
# One-time: nightly + cargo-fuzz
rustup toolchain install nightly
cargo install cargo-fuzz --locked  # or: brew install cargo-fuzz

# Build all targets
cd crates/modules/attestation/fuzz
cargo +nightly fuzz build

# Run a single target (15-second smoke)
cargo +nightly fuzz run borsh_call_message -- -max_total_time=15

# Reproduce a crash from the workflow's `fuzz-crashes-*` artifact
cargo +nightly fuzz run <target> artifacts/<target>/crash-<hash>
```

On macOS, `librocksdb-sys` (transitive through the SDK) needs libclang. If `cargo +nightly fuzz build` fails with a `libclang.dylib not found` error, set `LIBCLANG_PATH=/opt/homebrew/opt/llvm/lib` (after `brew install llvm`).

Found a crash? Two options:
1. Fix the bug inline in your PR. Add the crash input to a `corpus/<target>/` seed (cargo-fuzz auto-replays everything in `corpus/`).
2. File a separate issue with the input attached. Higher-severity crashes (state-tree corruption, panic in a public REST handler) are security-track and should follow [`SECURITY.md`](SECURITY.md) instead of public issues.

## Filing issues

Use the GitHub issue forms:

- **Bug report** — something broke; include the commit hash, OS, and a minimal repro
- **Feature request** — propose a new capability; include the user problem, alternatives considered, and a v0/v1/v2 scope guess
- **Question** — usually redirected to GitHub Discussions; only file as an issue if the answer needs a code change

Don't file a blank issue — the templates are there to keep the queue actionable.

## Security disclosures

**Do not file public issues for vulnerabilities.** See [`SECURITY.md`](SECURITY.md) for the disclosure policy. Short version: open a GitHub Private Security Advisory at <https://github.com/ligate-io/ligate-chain/security/advisories/new>, or email `hello@ligate.io` with subject prefix `[SECURITY]`.

Acknowledgement within 2 business days. 90-day coordinated disclosure default.
