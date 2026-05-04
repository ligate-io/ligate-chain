# Changelog

All notable changes to Ligate Chain are tracked here. Pre-launch (devnet target Q2 2026); no semver releases yet, everything sits under `[Unreleased]`. The first tagged release will fold the Unreleased contents into a versioned section at devnet boot.

Format follows [Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/). Entries group as **Added** (new functionality), **Changed** (behaviour or API shifts on existing functionality), **Fixed** (bugfixes), **Security** (advisories, supply-chain, fuzz), and **Deps** (transitive bumps not otherwise interesting). Issue and PR numbers reference [`ligate-io/ligate-chain`](https://github.com/ligate-io/ligate-chain).

This file is human-curated. Every PR adds an entry under `## [Unreleased]`; releases bump the buckets into a new `## [version] - YYYY-MM-DD` section.

## [Unreleased]

### Added

- Prometheus `/metrics` endpoint on `ligate-node` with five core metrics: schema / attestor-set / attestation success counters, `attestations_rejected_total{reason}` labelled by `AttestationError` discriminant, `state_db_size_bytes` gauge (on-disk allocation), `block_height` gauge polled from `LedgerStateProvider`, and `rpc_requests_total` + `rpc_request_duration_seconds` histograms via axum middleware. Process metrics auto-ship on Linux. Tracking issue [#110](https://github.com/ligate-io/ligate-chain/issues/110), Phase 1 in [#163](https://github.com/ligate-io/ligate-chain/pull/163), Phase 2 across [#167](https://github.com/ligate-io/ligate-chain/pull/167) [#168](https://github.com/ligate-io/ligate-chain/pull/168) [#169](https://github.com/ligate-io/ligate-chain/pull/169) [#170](https://github.com/ligate-io/ligate-chain/pull/170).
- `AttestationError::discriminant()` mapping all 18 variants to stable snake_case labels for the rejection counter, pinned by a unit test ([#167](https://github.com/ligate-io/ligate-chain/pull/167)).
- `cargo-deny` supply-chain gate enforcing license allow-list, source-allow-list, and duplicate version policy alongside the existing `cargo audit` ([#154](https://github.com/ligate-io/ligate-chain/pull/154), closes [#120](https://github.com/ligate-io/ligate-chain/issues/120)).
- `cargo-fuzz` harnesses for Borsh decoders (`CallMessage`, `Schema`, `AttestorSet`, `Attestation`, `SignedAttestationPayload`) plus Bech32m parsing and `AttestationId` round-trip. Nightly cron workflow with ~10 minute time budget. ([#156](https://github.com/ligate-io/ligate-chain/pull/156), closes [#119](https://github.com/ligate-io/ligate-chain/issues/119)).
- `.github/CODEOWNERS` with per-path review routing to `@ligate-io/maintainers` and `@ligate-io/protocol-reviewers` ([#173](https://github.com/ligate-io/ligate-chain/pull/173), closes [#113](https://github.com/ligate-io/ligate-chain/issues/113)).
- `docs/protocol/da-layers.md` documenting the DA-agnostic-by-construction architecture, surface inventory, and adapter playbook for adding a new DA backend ([#159](https://github.com/ligate-io/ligate-chain/pull/159), closes [#117](https://github.com/ligate-io/ligate-chain/issues/117)).
- `docs/protocol/threat-model.md` covering trust posture across the chain-id ladder, attestation-layer adversaries, sequencer-layer threats, wire-format runtime risks, cryptographic primitive failure, operational adversaries, and protection inventory ([#148](https://github.com/ligate-io/ligate-chain/pull/148), closes [#121](https://github.com/ligate-io/ligate-chain/issues/121)).
- `docs/protocol/rest-api.md` reference for all 114 auto-mounted REST endpoints with curl examples and Mermaid path map ([#147](https://github.com/ligate-io/ligate-chain/pull/147)).
- `docs/development/error-coverage.md` audit doc mapping every public `Error` variant to its test (and discriminant for `AttestationError`); audit pass added 3 new tests covering `InvalidAttestorPubkey`, `AttestationSignaturesOverCap`, `SchemaNameOverCap`, and tightened 14 existing `Reverted(_)` assertions to pin specific phrases ([#160](https://github.com/ligate-io/ligate-chain/pull/160), closes [#124](https://github.com/ligate-io/ligate-chain/issues/124)).
- `docs/development/celestia-ops.md` single-node Celestia runbook covering light-node provisioning, auth-token hygiene, env-var configuration, verification commands, and failure-mode triage ([#172](https://github.com/ligate-io/ligate-chain/pull/172)).
- `docs/protocol/upgrades.md` soft / hard fork policy, `CHAIN_HASH` derivation guard, pre / post-mainnet upgrade flow, change-classification table ([#109](https://github.com/ligate-io/ligate-chain/pull/109), closes [#44](https://github.com/ligate-io/ligate-chain/issues/44)).
- `/health` (liveness) and `/ready` (readiness) HTTP endpoints on the chain's REST surface for orchestrator probes. `/health` always returns 200; `/ready` returns 200 once `SyncStatus::Synced` and 503 with `{synced_da_height, target_da_height}` while catching up. State watched via a `tokio::sync::watch::Receiver<SyncStatus>` cloned from the SDK's existing channel. Closes [#176](https://github.com/ligate-io/ligate-chain/issues/176).
- Genesis validator hardening: `validate_config` now runs cross-module invariants on `attestation::AttestationConfig` to catch misconfigured `initial_attestor_sets` and `initial_schemas` at load time. Seven new typed `GenesisError` variants cover empty members, invalid threshold, duplicate attestor-set id, unknown attestor-set reference, duplicate schema id, orphan fee routing, and over-cap fee routing. 10 unit tests pin every variant. Closes [#175](https://github.com/ligate-io/ligate-chain/issues/175).
- Borsh wire-format snapshot tests for all public protocol types: `CallMessage`, `Schema`, `Attestation`, `AttestorSet`, `SignedAttestationPayload`, `AttestorSignature`, `AttestationId` ([#108](https://github.com/ligate-io/ligate-chain/pull/108), closes [#45](https://github.com/ligate-io/ligate-chain/issues/45)).
- E2E smoke test exercising the production runtime under `MockRollupSpec<Native>` against the auto-mounted REST surface, with state seeded through the production `AttestationConfig::initial_*` channels ([#145](https://github.com/ligate-io/ligate-chain/pull/145)).
- DA-isolation CI guard preventing Celestia coupling from leaking into DA-agnostic crates (`crates/stf`, `crates/stf-declaration`, `crates/modules`, `crates/client-rs`) via `scripts/check-da-isolation.sh` ([#118](https://github.com/ligate-io/ligate-chain/pull/118)).
- Chain-id string ladder locked in protocol spec, runbook, README, genesis example: `ligate-localnet`, `ligate-devnet-1`, `ligate-testnet-1`, `ligate-1` ([#105](https://github.com/ligate-io/ligate-chain/pull/105), refs [#54](https://github.com/ligate-io/ligate-chain/issues/54)).
- `#[non_exhaustive]` on every public protocol enum so future variant additions stay non-breaking for downstream consumers ([#103](https://github.com/ligate-io/ligate-chain/pull/103), closes [#43](https://github.com/ligate-io/ligate-chain/issues/43)).
- Research-primitive roadmap section in `upgrades.md` covering pre-mainnet vs post-mainnet integration story ([#135](https://github.com/ligate-io/ligate-chain/pull/135)).
- REST point-lookup specification in protocol docs: list and range queries deliberately live in the indexer service, not the chain ([#116](https://github.com/ligate-io/ligate-chain/pull/116), closes [#21](https://github.com/ligate-io/ligate-chain/issues/21)).

### Changed

- `attestation::max_builder_bps` migrated from a compile-time const to runtime state, governance-tunable per [#40](https://github.com/ligate-io/ligate-chain/issues/40)'s constants-to-state path. Default still `5000` (50%) for genesis-omitting configs. ([#152](https://github.com/ligate-io/ligate-chain/pull/152)).
- Devnet runbook (`docs/development/devnet.md`) refreshed to reflect post-Phase-A reality: actual binary, real config flags, current limitations table ([#107](https://github.com/ligate-io/ligate-chain/pull/107), closes [#84](https://github.com/ligate-io/ligate-chain/issues/84)).
- README Labs wordmark goes fully sage in the brand lockup, drops the 0.65-opacity treatment ([#106](https://github.com/ligate-io/ligate-chain/pull/106)).
- CI path-filter excludes `.github/dependabot.yml` so dependabot config edits don't trigger code-checking jobs ([#150](https://github.com/ligate-io/ligate-chain/pull/150)).
- Dependabot config ignores the schemars major and strum minor bumps that the SDK pin holds back ([#102](https://github.com/ligate-io/ligate-chain/pull/102)).

### Fixed

- `ligate_state_db_size_bytes` gauge now reports actual on-disk allocation via `meta.blocks() * 512` instead of nominal logical size via `meta.len()`. Caught during Tier 1 manual verification: gauge was reading 61 GB on a node with 23 MB actual disk usage because of NOMT and RocksDB sparse-file preallocation ([#171](https://github.com/ligate-io/ligate-chain/pull/171)).

### Security

- Audit ignore additions for transitive `astral-tokio-tar` advisories `RUSTSEC-2026-0112` and `RUSTSEC-2026-0113`, both reachable only through `sov-test-utils`'s `testcontainers` dev-dep, no production exposure ([#104](https://github.com/ligate-io/ligate-chain/pull/104)).

### Deps

- `actions/upload-artifact` bumped from v4 to v7 (dependabot, [#161](https://github.com/ligate-io/ligate-chain/pull/161)).
- `rustls` bumped from 0.23.39 to 0.23.40 (dependabot, [#162](https://github.com/ligate-io/ligate-chain/pull/162)).

[Unreleased]: https://github.com/ligate-io/ligate-chain/compare/main...HEAD
