# Changelog

All notable changes to Ligate Chain are tracked here. Pre-launch (devnet target Q2 2026). New work accumulates under `[Unreleased]`; tagged releases fold the contents into a versioned section. `v0.1.0-devnet` (2026-05-15) is the first such release.

Format follows [Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/). Entries group as **Added** (new functionality), **Changed** (behaviour or API shifts on existing functionality), **Fixed** (bugfixes), **Security** (advisories, supply-chain, fuzz), and **Deps** (transitive bumps not otherwise interesting). Issue and PR numbers reference [`ligate-io/ligate-chain`](https://github.com/ligate-io/ligate-chain).

This file is human-curated. Every PR adds an entry under `## [Unreleased]`; releases bump the buckets into a new `## [version] - YYYY-MM-DD` section.

## [Unreleased]

### Changed

- **Devnet-2 cutover (chain-id bump + stable-path adoption).** The public devnet rolls from `ligate-devnet-1` to `ligate-devnet-2`, with the AVOW denom and the new `lgt_token_id` → `avow_token_id` genesis JSON shape baked in. Mechanics: `devnet-1/` archived to `archive/ligate-devnet-1/` (the genesis files of the dead network kept for historical reference); a fresh `devnet/` created at the role-based stable path adopted in [#483](https://github.com/ligate-io/ligate-chain/pull/483), with `chain_id = "ligate-devnet-2"` in `celestia.toml` and `genesis_da_height` reset to `0` (operator pins to a live Mocha height at boot per `docs/development/runbooks/chain-id-bump.md`). Operator addresses (treasury, sequencer, demo wallets) unchanged from the previous devnet; balances reset to the same 1B AVOW initial distribution (900M / 50M / 50M). Cloud-init paths flip from `/opt/ligate/devnet-1/` to `/opt/ligate/devnet/`; the deployed GCE instance name, GCP Secret names, and GCS backup bucket are left as historical strings (operator renames at ops discretion). Grafana queries that filter on `chain_id = "ligate-devnet-1"` updated to `ligate-devnet-2`; dashboard UIDs (`/d/ligate-devnet-1-cost` etc.) preserved to avoid breaking deployed Grafana provisioning. Closes chain-id ladder bump from rung 1 → 2.
- **Release builds ~3-4x faster.** Two changes against `release.yml`'s `build:` matrix: (1) `[profile.release]` flips `codegen-units = 1` → `16` in `Cargo.toml`, restoring parallel codegen; (2) `release.yml`'s cache step swaps `actions/cache@v5` → `Swatinem/rust-cache@v2` (the same modern action `ci.yml` already uses), which auto-prunes `target/` to fit the GHA 10GB cache cap, scopes per-toolchain, and pushes warm-cache hit rate from ~50% to ~85%. Trade-off: pre-mainnet binaries lose ~2-5% inter-procedural optimization headroom. Flip `codegen-units` back to `1` (and likely `lto = "fat"`) at the first mainnet RC; release cadence slows then and binary performance starts to matter more than iteration speed. Cold release time on `v0.3.0` was ~25-30 min across the matrix; expected ~8-12 min after this change.

## [0.3.0] - 2026-05-25

Token rename + naming cleanup. The native token symbol moves from `$LGT` to bare `AVOW` (drops the leading `$` from the prose ticker per the cleaner convention), and the misnamed `devnet/` directory (actually the localnet config) becomes `localnet/`. Both are state-breaking on the wire and require a fresh devnet boot (`ligate-devnet-2`) to consume them. Mainnet is not yet live; this is the moment to do these renames cleanly. Cross-repo coordination tracked under [#457](https://github.com/ligate-io/ligate-chain/issues/457).

### Changed

- **Network directory restructure**: `devnet/` → `localnet/`. The old `devnet/` directory was actually the *localnet* config (MockDa, `chain_id = "ligate-localnet"`), confusingly named because it predated the `devnet-N/` numbered ladder. Rename adopts the role-based stable-path convention every mature chain uses (Solana, Sui, Aptos, Cosmos): the path describes what it is, the `chain_id` inside the TOML carries the network identity. Future network rollovers (devnet-2, testnet-1, mainnet-1) drop into siblings with stable paths; dead networks move to `archive/`. Test source files `crates/stf/tests/devnet_addresses.rs` and `crates/rollup/tests/devnet_config.rs` renamed to `localnet_*` to match. `devnet-1/` is unchanged (real public devnet, distinct concept). Touches 47 files; operators who scripted `cargo run ... --rollup-config-path devnet/rollup.toml` now use `localnet/rollup.toml`.
- **Token symbol renamed `$LGT` → `AVOW` everywhere**. New ticker semantically matches what the chain actually does (every attestation is an avowal). 4-char ticker, unclaimed across CoinGecko/CoinMarketCap/ETH/BSC/Solana. Substitutions cover both user-facing surface AND internal identifiers: `$LGT` prose → `AVOW`; on-chain `bank.gas_token_config.token_name` flips `"LGT"` → `"AVOW"`; base-unit `ulgt` → `uavow`; `nano-LGT` → `nano-AVOW`; genesis JSON field `lgt_token_id` → `avow_token_id` in both `localnet/genesis/attestation.json` + `devnet-1/genesis/attestation.json`; Rust types/fields `AttestationConfig::lgt_token_id` → `avow_token_id`, `AttestationModule::lgt_token_id` → `avow_token_id`, error variant `GenesisError::LgtTokenIdMismatch` → `AvowTokenIdMismatch`; REST endpoint `GET /v1/modules/attestation/state/lgt-token-id` → `/avow-token-id`. The chain stays **Ligate Chain**; the company stays **Ligate Labs**; bech32 address HRP stays `lig1...`. Brand identity does not change. Touches 36 files across `constants.toml`, `crates/`, `devnet*/`, `docs/`, `ops/grafana/`, `README.md`, `SECURITY.md`. **Breaking for `chain_hash` AND breaking on the wire**: genesis JSON shape changes (`lgt_token_id` field renamed), REST endpoint path changes, error variant name changes. Existing `ligate-devnet-1` runtime state cannot deserialize against this binary; a new devnet id (likely `ligate-devnet-2`) launches from clean genesis with `uavow` as the base denom. Mainnet is not yet live, so no migration concern there. Clients hitting `/lgt-token-id` need to update to `/avow-token-id`. Cross-repo follow-ups: `ligate-marketing#205`, `ligate-io/docs#23`, `ligate-research#100`, `ligate-api#68`, `ligate-js#41`, `ligate-cli#39`, `ligate-explorer#54`, `themisra-dashboard#33`. Closes [#457](https://github.com/ligate-io/ligate-chain/issues/457).

## [0.2.14] - 2026-05-23

Cost telemetry gets meaningfully accurate. `ligate_da_tia_burned_nano_estimate_total` now bumps by a per-blob estimate derived from the live Mocha gas price (already polled by the Celestia adapter) times a calibrated per-blob gas usage, instead of a fixed 300_000 nanoTIA constant. Plumbing release; no behaviour shift, `chain_hash` unchanged.

### Changed

- SDK pin bumped to `ligate-io/sovereign-sdk@ffff9df` (PR #5). The Celestia adapter now caches the live `gas_price` from its existing periodic stat poll and computes `fee_paid = ceil(gas_price * (PFB_BASE_GAS + bytes * PFB_GAS_PER_BYTE)) * 1000` nanoTIA on every `SubmitBlobReceipt`. Constants calibrated against Mocha for our payload shapes; gas price is live, so spikes get captured automatically.
- `ligate_da_tia_burned_nano_estimate_total` now reflects the estimated `fee_paid` from the receipt (via the `unwrap_or(constant)` fallback that's now hit only on mock-DA). For small attestation blobs the number is similar to v0.2.13; for larger proof blobs it's 10-15x more accurate.
- **Backup cadence: drop the daily tier; keep hourly (HOT, every 15 min) + weekly (COLD, Sundays 04:00 UTC).** The daily cold-snapshot at 03:30 UTC stopped `ligate-node` for ~60 s each day, costing one GCP-uptime-alert-fire per day under single-VM mode. It was redundant against the Sunday weekly + Celestia DA replay (the chain's actual source of truth). Net effect: 7 backup-induced downtimes/week → 1. Filed as chain#468; the proper "no downtime even on weekly" fix (RocksDB hot-checkpoint API) needs upstream NOMT work and stays in the backlog. Removed: `ops/backup/systemd/ligate-backup-daily.timer`. Updated: `docs/development/runbooks/backup-restore.md`, `docs/development/public-devnet-deploy.md`.

### What's still estimate-level

`fee_paid` is calibrated but not authoritative. The proper fix needs upstream `celestia-client` to expose a `get_tx` query (filed as [celestiaorg/lumina#974](https://github.com/celestiaorg/lumina/pull/974)) so we can read the real fee + `gas_used` straight off the PFB tx receipt. Once that lands, the adapter calls `state().get_tx(hash)` and `fee_paid` becomes the real value with no further chain-side changes.

### Compatibility

`chain_hash` unchanged from v0.2.13. Safe binary swap. The `ligate_da_blob_bytes_total` and `ligate_da_blobs_published_total` counters keep counting the same things. Only the `ligate_da_tia_burned_nano_estimate_total` rate changes (it gets more accurate).

## [0.2.13] - 2026-05-22

Hotfix for the v0.2.12 deploy panic on existing `blob_sender` RocksDB. Upgrading from v0.2.11 with a populated DB hit:

```
panicked at sov-blob-sender/src/db.rs:119:14:
Invalid blob info in the database: Error("missing field `size_in_bytes`", line: 1, column: 215)
```

### Fixed

- SDK pin bumped to `ligate-io/sovereign-sdk@2cd677a` (PR #4). The hotfix adds `#[serde(default)]` to the three new `SubmitBlobReceipt` cost fields (`fee_paid`, `gas_used`, `size_in_bytes`). Receipts persisted by older binaries now deserialize cleanly with sane defaults (`None`/`None`/`0`) instead of panicking. (chain#452)

### Operator notes

If you tried v0.2.12 and rolled back to v0.2.11, jump straight to v0.2.13. No data quarantine is needed; the `blob_sender` RocksDB rows that prompted the panic are read with `size_in_bytes = 0` on first load, then write the real value on the next `Published` event.

### Compatibility

`chain_hash` unchanged from v0.2.11/v0.2.12. Safe binary swap.

## [0.2.12] - 2026-05-22

Lands real on-wire bytes-per-blob telemetry from the DA layer (`ligate_da_blob_bytes_total`) by extending the SDK's `SubmitBlobReceipt` with cost fields. Plumbing release; no behaviour shift, `chain_hash` unchanged.

### Added

- `ligate_da_blob_bytes_total` (counter) — total bytes posted to the DA layer, summed from `SubmitBlobReceipt::size_in_bytes` on every observed `Published` event. Authoritative (not an estimate); lets Grafana show GB-per-month posted and real cost-per-byte trends as attestation payload size evolves. (chain#452)
- `ligate_da_blob_gas_total` (counter) — total DA-layer gas burned, summed from `SubmitBlobReceipt::gas_used`. Registered but does not bump yet: both Celestia (the celestia-grpc client's `TxInfo` carries only `hash` + `height`) and mock-DA leave `gas_used` as `None`. Once the Celestia `get_tx` follow-up surfaces gas this counter starts tracking real values with no code change here. (chain#452)

### Changed

- SDK pin bumped to `ligate-io/sovereign-sdk@33e1f419f` (PR #3). Extends `SubmitBlobReceipt<T>` with `fee_paid: Option<u64>`, `gas_used: Option<u64>`, and `size_in_bytes: u64`. `size_in_bytes` is always populated by every adapter; `fee_paid` + `gas_used` are `None` everywhere today pending the per-adapter follow-ups.
- `ligate_da_tia_burned_nano_estimate_total` now prefers the receipt's `fee_paid` when present (authoritative), falling back to the compiled-in `DA_BLOB_TIA_ESTIMATE_NANO` constant when the adapter leaves it `None`. The `_estimate_` suffix stays for now since the Celestia path still uses the constant; once the Celestia tx-body decode lands the suffix drops in a follow-up. (chain#452)

### Compatibility

`chain_hash` unchanged from v0.2.11. Safe binary swap. New counters appear automatically on next `/metrics` scrape.

### Followup

- Add a `get_tx` lookup in the Celestia adapter (after `submit_pay_for_blob` returns the `TxInfo`) to fetch the full `TxResponse`, extract `gas_used` and decode `tx.auth_info.fee.amount` for the utia coin. Once landed, both `ligate_da_blob_gas_total` and the `_estimate_` counter start emitting real Celestia values without further chain-side changes.

## [0.2.11] - 2026-05-22

Adds cost + economy Prometheus metrics so Grafana dashboards can show DA spend and protocol-treasury health alongside ops metrics. Plumbing-only release; no behaviour shift, `chain_hash` unchanged.

### Added

- `ligate_da_blobs_published_total` (counter) and `ligate_da_tia_burned_nano_estimate_total` (counter) — bump on every observed Celestia `Published` event. The TIA value is a **fixed-per-blob estimate** (`DA_BLOB_TIA_ESTIMATE_NANO = 300_000`, calibrated against Mocha rates on 2026-05-22), not extracted from the actual Celestia tx receipt. The SDK's `SubmitBlobReceipt` doesn't currently carry `fee_paid` from the PFB tx; making it do so is filed as a separate follow-up. Treat the gauge as a floor.
- `ligate_da_tia_estimate_per_blob_nano` (gauge) — exposes the compiled-in constant so Grafana can render "we assume X nanoTIA per blob" next to the burn counter.
- Six new gauges mirrored from api `/v1/stats/totals`: `ligate_protocol_treasury_balance_nano`, `ligate_protocol_total_supply_nano`, `ligate_protocol_txs_total`, `ligate_protocol_attestations_total`, `ligate_protocol_schemas_registered`, `ligate_protocol_attestor_sets_registered`. Polled every 60s by a new tokio task in `metrics.rs`. Soft-fail on api unreachable: gauges hold last value; `ligate_protocol_economy_last_scrape_unix` (also new) stops advancing so Grafana can flag staleness.
- `LIGATE_API_URL` env var (default `https://api.ligate.io`) for operators running their own api. Set to a non-existent host to effectively disable economy polling (gauges stay at zero / unset).
- `ops/grafana/ligate-node.json`: new **Cost & economy** row at y=48 with 4 panels: cumulative TIA burned (stat), TIA burn rate per hour (time series), treasury balance in LGT (stat), and a 4-up "protocol totals" stat row (txs / attestations / schemas / attestor sets).

### Compatibility

`chain_hash` unchanged from v0.2.10. Safe binary swap. New env var is optional with a sensible default.

### Followup

- File a chain issue to extend the SDK's `SubmitBlobReceipt` with the actual `fee_paid` from the Celestia PFB tx receipt. Once available, the TIA estimate becomes the real ledger.
- File an ops issue for GCP infra cost: enable Cloud Billing → BigQuery export and add a Grafana BigQuery datasource. Doesn't belong in `ligate-node` — the chain binary should stay portable for non-GCP operators.

## [0.2.10] - 2026-05-22

Surfaces the DbElected sequencer role + transitions as Prometheus metrics so the paper-leader scenario from 2026-05-21 (chain#446) becomes visible on Grafana, not just in logs. Plumbing-only change; no behaviour shift.

### Added

- `ligate_sequencer_role` gauge in `crates/rollup/src/metrics.rs`. 0 = unknown (chain HTTP unreachable / still booting), 1 = PgSyncReplica, 2 = BatchProducer. Sampled from the local `/v1/sequencer/role` endpoint every 2s (matches the SDK's Postgres heartbeat cadence).
- `ligate_sequencer_role_transitions_total{from,to}` counter. Bumps each time the sampled role differs from the previous sample. `from` and `to` labels are readable role names (`unknown` / `replica` / `leader`) so a Grafana panel can show "VM-3: replica → leader at 22:51 UTC" without a join.
- `crates/rollup/src/main.rs` spawns the role poller alongside the existing metrics tasks. Hardcoded loopback URL (`http://127.0.0.1:12346`); operators who change the SDK's HTTP bind would need to patch this, but nobody does in practice.
- `ops/grafana/ligate-node.json`: new **Cluster topology** row at y=40 with two panels — role per VM (Stat, value-mapped + color-coded: red unknown, blue replica, green leader) and role transitions rate (Time series over 5m windows). The role panel is the primary visual for catching split-brain or flapping.
- Unit tests in `metrics.rs` covering the `encode_role` shape (leader / replica / unknown on empty / unknown on future variant / whitespace handling).

### Operator notes

The Prometheus scrape config is unchanged. New metrics show up automatically on next scrape after the binary swap. Re-import `ops/grafana/ligate-node.json` to pick up the new row, or add the two PromQL queries to your own dashboard:

- `ligate_sequencer_role{instance=~"$instance"}`
- `sum by (instance, from, to) (rate(ligate_sequencer_role_transitions_total{instance=~"$instance"}[5m]))`

Alert idea (not shipped here, file an issue if you want a default rule): page on `count(ligate_sequencer_role == 2) != 1` sustained for 60s. Zero leaders = leaderless; two leaders = split-brain.

### Compatibility

`chain_hash` unchanged from v0.2.9. Safe binary swap.

## [0.2.9] - 2026-05-21

Removes the `--mode {sequencer,follower}` operator flag and the corresponding `NodeRole::Follower` machinery (chain#446). DbElected, the SDK's lock-elected sequencer mode, is the only sequencer path now; the boot-time mode toggle was redundant with the lock check and actively harmful in one observed case.

### Why

A paper-leader incident on 2026-05-21 18:53 UTC: VM-3 booted with `--mode follower` (set `automatic_batch_production = false` at startup), then acquired the Postgres leader lock via the in-process Replica→Leader role transition shipped in v0.2.6. `GET /v1/sequencer/role` returned `"BatchProducer"`, but the SDK's batch-production loop kept seeing the stale boot-time `false` and skipped every slot. The chain followed DA normally so slots advanced, but no user transactions made it into batches. Symptom externally: "attestations stuck in mempool, blobs not landing on Celestia for ~15 minutes." Resolved at 20:10 UTC by bouncing VM-3 (VM-1, which had no `--mode` flag, took the lock and resumed posting).

The fix in this release deletes the root cause rather than papering over it. In DbElected mode the lock is the producer gate; a separate "this node is a follower" flag is a footgun waiting for the next role transition.

### Removed

- `pub enum NodeRole { Sequencer, Follower }` from `crates/rollup/src/lib.rs`.
- `crates/rollup/src/follower_guard.rs` (Axum middleware that returned 503 on `POST /v1/sequencer/txs` for Follower-mode nodes). The SDK already returns 503 on submissions when a node doesn't hold the lock, so this middleware was redundant.
- `--mode` CLI flag from `crates/rollup/src/main.rs`. Operator invocation simplifies from `ligate-node --da-layer celestia --mode sequencer` to `ligate-node --da-layer celestia`.
- `node_role` field from `MockLigateRollup` and `CelestiaLigateRollup` structs.
- `new_with_role` constructors on both rollup blueprints; `new` is now the only path.
- The four `--mode` parsing tests in `main.rs`. Replaced with a single test that asserts `--mode` is rejected by clap (guard against re-introduction breaking operator scripts).

### Operator action

- VM-2 and VM-3 systemd units (`/etc/systemd/system/ligate-node.service`) had `--mode ${LIGATE_MODE}` stripped from their `ExecStart` line during the 2026-05-21 incident response. Already deployed; no further action needed for devnet-1.
- The `LIGATE_MODE` env var in `/etc/ligate/env` is now vestigial. Cloud-init writes it from instance metadata (chain#424/#425); it will be cleaned up in a follow-up operator change.
- Anyone using a fork or third-party deploy that passed `--mode follower` for a read-only node should drop the flag. The SDK's lock check handles the same protection at runtime.

### Compatibility

`chain_hash` unchanged from v0.2.8. Safe binary swap. CLI, JS SDK, explorer, and `api.ligate.io` all keep working with no client-side changes.

### Followup

- chain#447 (filed concurrently): proper `--no-sequencer` mode for read-only RPC nodes that don't instantiate the sequencer machinery at all. The previous Follower was a hybrid (sequencer instantiated, posting disabled by flag); the proper read-only path doesn't construct the sequencer in the first place.

## [0.2.8] - 2026-05-21

Adds the chain-side `/v1/cluster/nodes` REST endpoint (chain#442 Phase 1+2). Same chain code as v0.2.7 plus the new cluster module and a Caddyfile entry that keeps the endpoint off the public RPC. `chain_hash` unchanged. Safe binary swap.

### Added

- `crates/rollup/src/cluster.rs`: `GET /v1/cluster/nodes` returns the live DbElected cluster topology (every node that has heartbeated into Postgres, plus which one currently holds the leader lock). 1-second in-memory cache. Returns 503 with `{"reason": "not_clustered"}` on legacy single-sequencer nodes. Wired into both `celestia_rollup.rs` and `mock_rollup.rs`.
- `ops/caddy/Caddyfile`: 404 block for `/v1/cluster/nodes`, mirroring the existing `/v1/sequencer/role` and `/metrics` blocks. Response contains private VPC addresses; only internal callers (Grafana, ops dashboards, the future api proxy) should hit the chain directly.

### Changed

- Internal workspace crates bumped 0.2.7 -> 0.2.8 in `Cargo.lock`.
- `Cargo.toml`: adds `sov-full-node-configs` to workspace deps (covered by the existing `[patch]` table for the SDK fork).
- `crates/rollup/Cargo.toml`: adds `sqlx 0.8` (same version as the SDK fork's `sov-sequencer`) and `sov-full-node-configs`.

### Compatibility

`chain_hash` unchanged from v0.2.7. Safe binary swap. CLI, JS SDK, explorer, and `api.ligate.io` all keep working with no client-side changes.

The Caddyfile update is operator-side (not auto-deployed by the release tarball). After binary swap on each VM, run `sudo cp ops/caddy/Caddyfile /etc/caddy/Caddyfile && sudo systemctl reload caddy` on the public-facing VM (VM-1 on devnet-1).

### Followup (chain#442)

- Phase 3: `ligate-api` proxy at `api.ligate.io/v1/cluster/nodes` that strips private addresses.
- Phase 4: `ligate-explorer` Cluster page consuming the api proxy.

## [0.2.7] - 2026-05-21

Hotfix for v0.2.6 which was broken on deploy: the HTTP server died 20 seconds after boot because of a faulty `tokio::time::timeout` wrap around `axum::serve` in the SDK fork. Reverts that wrap. The chain-side 35-second shutdown watchdog (also added in v0.2.6) still bounds any future shutdown hang. v0.2.6 was deployed to VM-1 at 10:52 UTC and rolled back to v0.2.4 at 10:55 UTC after the public RPC started returning 502; no chain state was affected.

### Fixed

- SDK fork (`ligate-io/sovereign-sdk@2633af2d5`): revert `tokio::time::timeout(20s, axum::serve(...).with_graceful_shutdown(...))` from `sov-stf-runner/src/http/mod.rs`. The previous code in v0.2.6 forced the HTTP server to return Ok after 20 seconds of normal operation regardless of whether shutdown was signaled, leaving the chain producing batches but with no REST endpoint. The intent (bound graceful drain to 20s after shutdown signal) would need to spawn a separate watchdog task; for now the chain-side process-level watchdog at 35s covers the original hang scenario.
- Kept the 5-second timeout on `server_handle.stopped()` after shutdown signal (jsonrpsee server drain); that one is correctly gated by the prior shutdown call so it can't fire during normal operation.

### Changed

- SDK pin bumped from `d3e7acf28e1ebe98f523c235084c61a1c93d0b38` to `2633af2d599c6a899fcdc484a0f198a26307d16b`.
- Internal workspace crates bumped 0.2.6 -> 0.2.7 in `Cargo.lock`.

### Compatibility

`chain_hash` unchanged from v0.2.6 (and v0.2.5). Safe binary swap.

## [0.2.6] - 2026-05-21

Failover-recovery hardening (chain#435). Cuts a tagged release for the SDK pin bump and shutdown-watchdog plumbing that resolve the three bugs surfaced by the 2026-05-20 multi-sequencer failover drill: a forced kill on the leader left every node in a cascading 5-minute shutdown hang with stale Postgres state, requiring manual RocksDB surgery to recover. The combined fix in this release replaces the SDK's "Replica acquired leadership → exit-to-restart" pattern with an in-process role transition, makes graceful shutdown bounded, and lets any future kill auto-recover.

### Added

- `crates/rollup/src/main.rs`. 35-second shutdown watchdog. On first SIGTERM/SIGINT/SIGQUIT, arms a hard-exit deadline; if `main()` hasn't returned by then, calls `std::process::exit(0)`. Complements (not replaces) the SDK's signal handler; insurance against any future task that fails to honour the cancellation cascade. Verified locally against MockDa: SDK shutdown took longer than 35s on a clean SIGTERM, watchdog brought the process down at the deadline as designed.
- SDK fork (`ligate-io/sovereign-sdk@d3e7acf28`): `AtomicSequencerRole` wrapper (`Arc<AtomicU8>`), `Message::{PromoteToLeader, DemoteToReplica}` actor variants, `RoleTransitionRequester` trait + impl on `SequencerStateUpdator<S, Rt>`, `RoleTransitionRequest` mpsc between actor and `SideEffectsTask`, `LeaderConstructionDeps` stash on `SideEffectsTask`, `PreferredBlobSender::activate_leader_state` / `deactivate_leader_state`, `PreferredSequencerDb::set_backend`. Net effect: a DbElected replica that wins the Postgres lock seamlessly transitions to `BatchProducer` in-process. no process exit, no systemd restart.

### Changed

- SDK pin bumped from `4b4a313b7d89cf7ca4b37edd6cefec4123d51add` to `d3e7acf28e1ebe98f523c235084c61a1c93d0b38` across all 32 `[patch."https://github.com/Sovereign-Labs/sovereign-sdk.git"]` entries in `Cargo.toml`. Includes Bug 1 (sequence-number panic-to-reset), Bug 2a (HTTP server 20s/5s timeouts), and Bug 3 Phases 1–4 (in-process role transition).
- Internal workspace crates (`attestation`, `ligate-bootstrap-cli`, `ligate-client`, `ligate-genesis-tool`, `ligate-prover-risc0`, `ligate-rollup`, `ligate-stf`, `ligate-stf-declaration`) bumped 0.2.5 → 0.2.6 in `Cargo.lock` to match the workspace version.

### Fixed

- **chain#435 Bug 1**: SDK fork `sov-sequencer/src/preferred/db/mod.rs:291` no longer panics when local sequencer state (Postgres `in_progress_batch` row OR RocksDB equivalent) is ahead of DA's confirmed view. The assertion is replaced with `tracing::error!` + early-return empty `Vec` from `proofs_for_replay`; `initial_data` detects the stale row condition on boot and discards it in-memory (next `begin_rollup_block` UPSERTs over it via `ON CONFLICT (singleton) DO UPDATE`). Without this fix, any forced kill of the leader mid-batch left the node in an unrecoverable crash loop until an operator wiped `preferred_sequencer/` by hand.
- **chain#435 Bug 2a**: SDK fork `sov-stf-runner/src/http/mod.rs` wraps `axum::serve(...).with_graceful_shutdown(...)` in `tokio::time::timeout(20s, ...)` and `server_handle.stopped()` in `tokio::time::timeout(5s, ...)`. Without these a single keepalive connection (e.g. Caddy reverse proxy) blocked process exit until systemd's `TimeoutStopSec` (5 min default) elapsed.

### Compatibility

`chain_hash` unchanged from `v0.2.5`. No state changes. No re-genesis. Safe binary swap on existing `devnet-1` VMs.

In single-sequencer legacy mode (`celestia.toml` without a `[sequencer.preferred.postgres_config]` block. current VM-1 state), all of this code is dormant. The Bug 1 + Bug 2 watchdog are the operational wins: any future kill auto-recovers without manual RocksDB surgery, and worst-case shutdown is bounded at 35s.

In DbElected multi-sequencer mode (current VM-2 + VM-3. stopped), the SDK now supports in-process role transitions. **Re-enabling DbElected on the existing cluster is now safe**; the failover drill that previously caused a 26-minute outage should complete cleanly with PID-stable role flips.

### Known limitations

- A leader that gets demoted to replica in-process does NOT yet have its `ReplicaSyncTask` started if it booted as a leader. The `replica_task` is constructed unconditionally in SDK `initialization.rs` but only `start()`ed in the boot-time `if let PgSyncReplica` branch. So a demoted leader catches up via DA only (slower) until it next gets promoted back. Tracked as a follow-up; not blocking for devnet-1's 1-leader-2-cold-standby topology where demotions are rare.
- The `BlobSender` `JoinHandle` returned from `activate_leader_state` is not tracked in the SDK's `background_handles` vec. It lives until the global shutdown signal fires. Adequate for our purposes.

### Followup (out of scope for v0.2.6)

- `ops/cloud-init/sequencer.yaml`. bump `LIGATE_TAG` from `v0.2.5` to `v0.2.6` so future VM (re-)images pick up the fix. Follow-up PR, same pattern as v0.2.5 ops bump.
- chain#82. multi-sequencer umbrella. Schedule re-enabling DbElected on VM-2 + VM-3 + re-running the forced-failover drill once v0.2.6 is deployed and soaked on VM-1.
- ReplicaSyncTask hot-start on demotion (see Known limitations above).

## [0.2.5] - 2026-05-21

Ops + docs patch. No chain code changes; `chain_hash` unchanged from `v0.2.4`; safe binary swap in place. Cuts a tagged release so the multi-instance cloud-init work (PRs #424 + #425), the canonical `devnet-1/` substituted bundle (#429), and the multi-instance Grafana dashboard (#431) ship as a pinned point-in-time bundle. The cloud-init's `LIGATE_TAG` pin advances to `v0.2.5` in a follow-up so VM-4 (and any future re-image) boots fully clean from `git clone` + Secret Manager fetch, no manual operator intervention.

### Added

- `ops/cloud-init/sequencer.yaml` — multi-instance support (#424, #425). Reads `LIGATE_NODE_ID` + `LIGATE_MODE` per VM from GCP instance metadata; auto-installs the `ligate-node` binary from the GitHub release; auto-places `/opt/ligate/devnet-1/` from the chain repo at the pinned tag; fetches the Celestia DA signer key from GCP Secret Manager into `/etc/ligate/env.d/celestia-signer` (drop-in dir so cloud-init's managed `/etc/ligate/env` and operator-supplied secret material don't clobber each other); installs Grafana Alloy with config templated for the per-instance `instance` label so per-VM metrics flow into Grafana Cloud Mimir distinguishably. Five iteration-round bugs hit during the live VM-2 first boot (binary mkdir, sha256 filename, Celestia RPC JWT, signer requirement under follower mode, Alloy missing) all fixed inline with comments explaining why.
- `ops/cloud-init/render-rollup-toml.sh` (shipped in v0.2.4, unchanged here) — per-instance rollup.toml renderer for the DbElected multi-sequencer activation. Still ready to plug in at cutover; current VMs run as `--mode follower` without activating the postgres_config block.
- `ops/grafana/ligate-node.json` — `$instance` template variable + filter rewriting on all 17 ligate_*/process_* metric queries (#431). Multi-select, defaults to "All". Lets the dashboard separate per-VM time series cleanly now that VM-2 + VM-3 are emitting alongside VM-1.

### Changed

- `devnet-1/genesis/chain_state.json` — replaces the template `"genesis_da_height": 0` with the live `"genesis_da_height": 11357228` (#429). 11357228 is the Mocha-4 block height at devnet-1 launch; previously this value lived only on the operator-substituted production VM-1. Committing it makes the repo's `devnet-1/` directory the canonical live-deployment bundle (anyone with the public repo + a Celestia signer key can run a working follower) rather than a tool-substitutable template.
- `devnet-1/celestia.toml` — `prover_address` + `rollup_address` switched from the template-substituted placeholder (`lig1zd9j2z6...`) to the v0 Anvil dev actor address (`lig1h72nh5c7...`) that's documented in `devnet/rollup.toml` (the localnet config) and used in production by VM-1. Same canonical-bundle rationale as the chain_state.json change.
- `README.md` — full "macOS first-build trifecta" callout under "System dependencies" plus a Quick start pointer at the published GHCR image and GitHub release binaries (#423, #417, #418). New macOS contributors now hit one self-contained recipe instead of bouncing between README and CONTRIBUTING.

### Deps

- No `Sovereign-Labs/sovereign-sdk` rev change. `[patch]` block still pins `ligate-io/sovereign-sdk@4b4a313b7` (same as v0.2.4).
- Internal workspace crates (`attestation`, `ligate-bootstrap-cli`, `ligate-client`, `ligate-genesis-tool`, `ligate-prover-risc0`, `ligate-rollup`, `ligate-stf`, `ligate-stf-declaration`) bumped 0.2.4 → 0.2.5 in `Cargo.lock` to match the workspace version.

### Compatibility

`chain_hash` unchanged from `v0.2.4`. No state changes. No re-genesis required. Operators deploying this binary to existing `devnet-1` VMs keep their RocksDB state intact across the swap. The `devnet-1/genesis/chain_state.json` change is config-only (the indexer + chain read this at boot but it's not part of the chain_hash computation).

### What this unlocks downstream

- VM-4 (or any future re-image of VM-1/-2/-3) booting fully clean from cloud-init alone — no SCP of substituted bundle from VM-1, no manual chain_state.json patch
- The next cutover step under chain#422 ("multi-sequencer prod deploy"): VM-2 + VM-3 are already running on this release as followers; the cutover script activates the DbElected postgres_config block via `render-rollup-toml.sh` + restarts ligate-node on all three with `--mode sequencer`

### Followup (out of scope for v0.2.5)

- chain#426 — refactor follower mode to skip `PreferredSequencer` init so the Celestia signer dependency goes away. Multi-day SDK fork patch; tracked for v1+ ergonomics. The current workaround (signer placed on every VM via Secret Manager) is operationally fine for our internal multi-sequencer setup.
- chain#392 — GCP cost panels (VM-hours, SSD storage, network egress) in Grafana. Still open; needs BigQuery billing export + Grafana data source work. Multi-hour, separate session.

## [0.2.4] - 2026-05-20

Ops + docs patch. No chain code changes; `chain_hash` unchanged from `v0.2.3`; safe binary swap in place. Cuts a tagged release so the multi-sequencer activation artifacts (design doc + plumbing audit findings + rollup.toml config seam + per-instance render script + Docker smoke harness) ship as a pinned point-in-time bundle alongside the Dependabot bumps that landed since `v0.2.3`. Sub-issue 5 (multi-sequencer prod deploy) consumes the `v0.2.4` artifacts (release tarball + GHCR image) rather than tracking a moving `main`.

### Added

- `docs/protocol/sequencer-decentralisation.md` — design doc deciding the multi-sequencer path: Option 1 (internal Postgres-elected leader rotation) → Option 2 (federated validator set, pre-mainnet). Out of scope: shared sequencer (Astria/Espresso/Skip, fails the PoUA-compatibility requirement) and based rollup (Celestia validators sequence, latency-bound). 5 open questions explicitly deferred; revisit triggers named. (#409)
- Plumbing-audit findings folded into the same doc's §5: the Sovereign SDK pin (`ligate-io/sovereign-sdk@4b4a313b7`) already implements `ConfiguredNodeRole::DbElected` end-to-end (heartbeat task, replica sync, Postgres-backed election, SQL `is_leader()` function). What was originally a 6-sub-issue plan over 6-8 weeks collapses to a 4-sub-issue plan over ~2-3 weeks; the bulk of remaining work is provisioning + Caddyfile + Grafana labels, not Rust. `crates/rollup/src/lib.rs` gains a `// SEAM:` doc-comment block pointing readers at the SDK's existing implementation. (#410, #411)
- `devnet/rollup.toml`: commented `# SEAM:` block under `[sequencer.preferred]` documenting how to activate `DbElected` multi-instance mode by uncommenting and supplying `postgres_connection_string` + `node_id` + `node_role = "DbElected"`. Block stays commented in the canonical prod config (single-instance) until sub-issue 5 spins up the second and third VMs. (#416)
- `ops/cloud-init/render-rollup-toml.sh`: per-instance render script. Reads the canonical `devnet/rollup.toml`, fetches the Cloud SQL app-role password from GCP Secret Manager (`cloudsql-ligate-sequencer-db-app`) via the VM service account, substitutes `${LIGATE_NODE_ID}` + `${POSTGRES_PASSWORD}` into the SEAM block, writes the activated config to `/var/lib/ligate/rollup.toml` at mode 600. Marker-anchored sed transforms refuse to run on a source toml that's missing the `# SEAM:` header (schema-drift guard). Idempotent: same env in → same output. Designed to run as a systemd `ExecStartPre=` step. (#416)
- `tests/smoke/multi-sequencer/`: Docker Compose smoke harness validating the SDK's DbElected mechanism. Postgres + 3× chain instances (`ghcr.io/ligate-io/ligate-chain:0.2.3`) booting against shared MockDa SQLite and shared Postgres. Live-verified: exactly one `BatchProducer` + two `PgSyncReplica` per run, `GET /v1/sequencer/role` returns the correct value on each endpoint, leader-kill is observed by survivors via the SDK's heartbeat task. Failover capture (survivor flipping to `BatchProducer`) is best-effort on MockDa due to multi-writer SQLite contention — the audit-claim core (election + heartbeat-observation) is fully verified; survivor-takeover is tracked separately for upgrade to local-celestia-devnet (#420). Five macOS-Docker gotchas hit and documented inline: GHCR tag drops the `v` prefix, minimal image lacks `envsubst`/`wget`/`curl` (sed + host-side polling instead), named volumes are root-owned (`user: "0:0"`), io_uring blocked by default seccomp (`seccomp=unconfined`), SDK 10s grace period races MockDa crash (smoke-only override to 500ms). (#419)

### Deps

- `Sovereign-Labs/sovereign-sdk` rev pin in `[patch]` block unchanged from `v0.2.3` (`4b4a313b7`). All DbElected machinery already in that pin; no SDK bump needed. Workspace pins in `[workspace.dependencies]` stay at `f6cd64749e…`.
- `getrandom` bumped from `0.3.4` to `0.4.2` (Dependabot #402). Transitive dep; no code path changes needed. `cargo audit` / `cargo deny` / `cargo test` all green.
- `actions/upload-artifact` bumped `4` → `7`, `actions/download-artifact` bumped `4` → `8`, `docker/login-action` bumped `3` → `4` in CI workflows (Dependabot #399, #400, #401). CI surface only; no chain binary effect.
- `cargo audit` ignore list extended with `RUSTSEC-2026-0145` (`astral-tokio-tar` 0.6.1 PAX header desync; transitive via `testcontainers` in `sov-test-utils` dev-deps; test-only path, not reachable from production code; solution upgrade to >= 0.6.2 — clears when SDK testcontainers bumps).

### Compatibility

`chain_hash` unchanged from `v0.2.3`. No state changes. No re-genesis required. Operators deploying this binary to an existing `devnet-1` VM keep their RocksDB state intact across the swap.

## [0.2.3] - 2026-05-18

Patch release. Adds `da_block_height` to the batch receipt JSON for explorer-side Celenium deep-links. Backward-compatible additive change; `chain_hash` unchanged; safe binary swap in place.

### Added

- `receipt.da_block_height: Option<u64>` on the batch receipt JSON returned by `GET /v1/ledger/batches/{id}` and the slot-nested variants. `Some(height)` for receipts produced by the node-side `apply_slot` replay path (where the slot header is in scope and `header.height()` is the Celestia inclusion height for the batch's blob); `None` for receipts produced by the sequencer's optimistic pre-inclusion path (height not yet known at execution time). The canonical receipts indexed off the ledger DB are always the node-side ones, so consumers (api / explorer) reliably get `Some(height)`. Powers the upcoming explorer "View on Celenium" deep-link per ligate-io/ligate-chain#355.
- Locally verified against Mock DA: `receipt.da_block_height` reliably appears in batch JSON, matching the slot number under Mock DA's 1:1 slot↔DA-height mapping.

### Deps

- `[patch]` SDK pin bumped to `ligate-io/sovereign-sdk@4b4a313b7` (branch `ligate/da-block-height-on-batch-receipt`, on top of the v0.2.2 openapi commit). Single-commit SDK change adds `Option<u64> da_block_height` to `BatchSequencerReceipt<S>` (with `#[serde(default)]` for backward-compat) and threads it through the `apply_slot` → `apply_batches_in_user_space` → `apply_batch` (registered + unregistered) call chain; sequencer-side optimistic block executor passes `None`.

### Compatibility note

The `Option<u64>` + `#[serde(default)]` shape means batch receipts written before this binary swap deserialise cleanly as `da_block_height: null`. No re-genesis. No state wipe. In-place binary replacement is the entire upgrade procedure.

## [0.2.2] - 2026-05-18

Same-day patch on top of v0.2.1. Two leftover bugs from the v0.2.1 verification pass on prod, both now fixed for real. `chain_hash` unchanged; safe binary swap in place.

### Fixed

- **OpenAPI `info.title` + `info.description` now override correctly.** v0.2.1's #395 fix at `crates/stf/src/runtime.rs` was diagnostically correct (direct field assignment instead of `Info::new(...)`), but couldn't possibly land because the Sovereign SDK's `register_endpoints()` in `sov-modules-rollup-blueprint/src/native_only/endpoints.rs:88-92` unconditionally re-overwrote both fields with the SDK defaults right after calling our `Runtime::openapi_spec()`. The other three fields (`version`, `contact`, `license`) survived because they weren't touched. Verified live: post-v0.2.1 swap, `curl https://rpc.ligate.io/v1/openapi-v3.json | jq .info` returned `title: "Sovereign SDK Rollup JSON API"` with our version/contact/license correctly applied, confirming the runtime override was being called but immediately stomped. Patched in our SDK fork (`ligate-io/sovereign-sdk@09068b6f`): the SDK now only falls back to its defaults when `runtime_spec.info.title.is_empty()` / `info.description.is_none()`, respecting whatever the runtime set. Cargo `[patch]` block bumped from `eab3f9d0` → `09068b6f3`. Closes ligate-io/ligate-chain#393 (this time for real; v0.2.1 closed it prematurely).
- `ops/exporters/celestia-tia/celestia-tia-exporter.py`: `LIGATE_READY_URL` default changed from `http://127.0.0.1:12346/v1/health/ready` (which 404s) to `http://127.0.0.1:12346/ready`. The chain exposes health endpoints at the root, not under `/v1/` (path inventory: `/health` returns `{"status":"ok"}`, `/ready` returns `{"status":"synced","synced_da_height":N}`, `/v1/healthcheck` returns a literal `ok` string). Picked `/ready` because it actually reports sync status. With the wrong URL, the `ligate_ready` gauge always emitted 0 — the `ReadyFalse` alert in `ops/prometheus/alerts.yaml` would have fired the moment alert evaluation started against this metric.

### Deps

- `Sovereign-Labs/sovereign-sdk` rev pin in `[patch]` block bumped from `eab3f9d0ae...` to `09068b6f3d...` (the openapi `info` fix, on top of `ligate-mainline`). Upstream-declared pins in `[workspace.dependencies]` stay at `f6cd64749e...` (those are identity declarations that the `[patch]` redirects; cargo doesn't fetch them).

## [0.2.1] - 2026-05-18

Cosmetic + operator-UX + ops-tooling patch release. `chain_hash` unchanged; safe binary swap in place. Bundles the openapi `info`-block fix, the swagger-ui v5 swap (both #393/#394), plus three operator-side additions that make devnet-1 reproducible: live Caddyfile committed, swagger-ui installer scripted, TIA exporter extended with chain-readiness and a dead-man's switch.

### Added

- `ops/caddy/Caddyfile`: live `/etc/caddy/Caddyfile` from the devnet-1 sequencer VM committed to repo as the canonical source. Documents the Cloudflare-only edge allowlist (defense-in-depth against direct-to-origin IP scans), the openapi-v3.json path rewrite (browser hits `/openapi-v3.json` without `/v1/` prefix; Caddy rewrites internally), the swagger-ui v5 disk-served override, and the `/metrics` lockdown. Operators rebuilding the VM now reach for the repo's Caddyfile instead of reading state off a running VM. Drop the swagger-ui v5 override block once Sovereign SDK upgrades its bundled UI. Closes ligate-io/ligate-chain#394 (companion to the docs update below).
- `ops/install/swagger-ui-dist.sh`: idempotent installer for swagger-ui-dist@5.17.14 into `/var/www/swagger-ui/`. Downloads the upstream tarball, strips two path components so `dist/*` lands at the install root, writes a custom `swagger-initializer.js` that points the bundle at `/openapi-v3.json` with `tryItOutEnabled: true`, chowns to `caddy:caddy` (falls back to `chmod a+r` if the user isn't present on the host). Env-configurable for `SWAGGER_UI_VERSION`, `INSTALL_DIR`, `OWNER_USER`, `OWNER_GROUP`, `SPEC_URL`. Replaces the manual `curl | tar` recipe in the runbook. Same #394 cleanup track.
- `ops/exporters/celestia-tia/celestia-tia-exporter.py`: new `ligate_ready{instance}` gauge by polling the chain's `/v1/health/ready` (default `http://127.0.0.1:12346/v1/health/ready`, 3s timeout) on the same cadence as the balance poll. Gives the `ReadyFalse` alert in `ops/prometheus/alerts.yaml` a metric to fire on without standing up a blackbox-exporter. Companion gauge `ligate_ready_last_check_seconds` exposes the last-probe timestamp for staleness debugging.
- `ops/exporters/celestia-tia/celestia-tia-exporter.py`: dead-man's-switch gauge `celestia_tia_exporter_alive{instance,account}` flips to 0 if no balance poll has succeeded in `DEAD_MANS_SWITCH_SEC` (default 300s). Lets alert rules separate "balance is low" from "we have no idea what the balance is" (the latter usually means the celestia-light node is dead, not the wallet).
- `ops/exporters/celestia-tia/celestia-tia-exporter.py`: per-account `account` label (default `da-signer`, configurable via `DA_SIGNER_ACCOUNT` env) on `celestia_node_da_signer_balance_utia` + the three exporter-internal gauges. Future-proofing for the multi-signer split (hot/cold or rotation); today it's a single value, but dashboards group by friendly name instead of by a 45-char bech32. Follow-up #397 tracks the chain-side `ligate_da_submission_failures_total` counter the SDK has to emit (out of scope for this exporter).

### Fixed

- `crates/stf/src/runtime.rs`: openapi-spec `info` block title + description now actually override (previously only version/contact/license persisted while title + description silently reverted to the Sovereign SDK defaults "Sovereign SDK Rollup JSON API" / "Sovereign SDK Runtime, Ledger and Sequencer JSON API"). Root cause: `Info::new(title, version)` constructor's title field was clobbered downstream; switching to direct field assignment (`spec.info.title = "..."` etc.) starting from the existing info struct makes every override stick. The served spec now correctly identifies the chain as Ligate Chain across every info field. Closes ligate-io/ligate-chain#393.
- `docs/development/public-devnet-deploy.md`: Caddy reverse-proxy section now documents serving swagger-ui-dist@5+ from `/var/www/swagger-ui/` at `/v1/swagger-ui/*` instead of proxying the chain's bundled bundle. The chain's bundled swagger-ui ships an older build that refuses to render any spec declaring `openapi: "3.1.0"`, which is what utoipa hardcodes (single-variant enum, can't be downgraded from the chain code). The Caddy override loads a 5+ bundle from disk that renders 3.1 specs natively, restoring the `/v1/swagger-ui/` endpoint. Drop this Caddy override once Sovereign SDK upgrades its bundled swagger-ui. Closes ligate-io/ligate-chain#394.

## [0.2.0] - 2026-05-17

**Breaking wire-format change.** `AttestationId` is now a single 32-byte hash with the `lat` bech32m prefix (`lat1…`), replacing the pre-v0.2.0 compound `<schema_id>:<payload_hash>` (`lsc1…:lph1…`) form. The on-chain `StateMap<AttestationId, Attestation<S>>` key encoding therefore changes, which means **this release requires a re-genesis** on any chain that has already submitted attestations. devnet-1 is being re-genesised as part of this cut; no historic devnet state survives.

`AttestationId::from_pair(schema_id, payload_hash)` is the canonical constructor: computes `SHA-256(schema_id_bytes ‖ payload_hash_bytes)` and wraps the result in the macro-generated `lat`-prefixed newtype. The `(schema_id, payload_hash)` components remain queryable: they are still stored as fields on the `Attestation<S>` value returned by `GET /v1/modules/attestation/attestations/{lat1…}`.

Why this ships now: the v0.1.x compound id was the odd duck in an otherwise uniform `lsc`/`las`/`lph`/`lpk`/`lat` typed-id family (every other Ligate identifier is a single bech32m string). The compound form was hand-rolled around a Serde-blanket-impl gap on `[u8; 64]`; switching to the 32-byte hash collapses display width by half (~60 chars vs ~120), drops the `:` URL-encoding edge case, and gives partners a single regex to validate against. The migration cost is fully borne pre-public-announce; no external partner has hit live devnet yet.

### Changed

- `crates/modules/attestation/src/lib.rs`: `AttestationId` replaced with the macro-generated newtype `mod attestation_id_mod { impl_hash32_type!(AttestationId, AttestationIdBech32, "lat"); }`. `from_pair` now derives a 32-byte SHA-256 digest of `schema_id ‖ payload_hash` instead of storing the two halves as separate struct fields. Display becomes `lat1…`. The old `Display::fmt` (`{schema_id}:{payload_hash}`) + hand-rolled `FromStr` / `JsonSchema` impls are gone (the macro provides the equivalents).
- `crates/modules/attestation/src/query.rs`: route doc updated; the `Path<AttestationId>` extractor now expects a single `lat1…` segment (the underlying `AttestationId::from_str` is macro-generated bech32m).
- `crates/modules/attestation/tests/borsh_snapshot.rs`: `AttestationId` snapshot updated to the new 32-byte hash (`b0dcb09af5496e779e60b21109a718475091191efc7a8638b01d51c622fc9128` = `SHA-256([0x11; 32] ‖ [0x33; 32])`). Any wire-format drift will break this test.
- `crates/modules/attestation/tests/unit.rs`: replaced the field-access invariants (`id.schema_id == sid`, `id.payload_hash == ph`) with a determinism check (matches direct `Sha256::new().update(...)` computation) and a position-sensitivity check (`from_pair(a, b) != from_pair(b, a)` catches collisions across mirrored inputs).
- `crates/rollup/tests/binary_spawn_smoke.rs`: REST polling URL now uses `format!("{base}/v1/.../attestations/{id}", id = AttestationId::from_pair(...))` instead of literal `{schema_id}:{payload_hash}` interpolation.
- `docs/protocol/attestation-v0.md`: appendix REST table + section explaining the new `lat1…` id derivation.
- `crates/modules/attestation/fuzz/fuzz_targets/{rest_attestation_path,attestation_id_parse}.rs`: harness module docs updated to describe the bech32m single-id parse surface.

### Migration

Re-genesis required on any node that booted against `v0.1.x` and has at least one attestation in state. Steps documented in `docs/development/runbooks/chain-id-bump.md`; in practice for devnet-1: stop the node, wipe `LIGATE_STORAGE_DIR`, restart with the new binary against the same `devnet-1/genesis/` (genesis bytes unchanged), re-run the bootstrap-cli ceremony to re-register canonical schemas + attestor sets. `chain_hash` MAY shift if the state-tree's empty-map type identity is part of the genesis root computation; verify against the post-restart `/v1/rollup/info` output and re-pin `constants.toml` accordingly.

## [0.1.3] - 2026-05-17

Same-day follow-up to v0.1.2. Four PRs: cold-backup downtime fix, swagger UI wiring, OpenAPI spec branding override, and a prose-style scrub (em dashes out across the docs landed in v0.1.2). `chain_hash` unchanged; binary swap is in-place.

**State-preservation note.** `attestation.json` / genesis bytes / `chain_hash` all unchanged from `v0.1.2`. Operators deploying this binary keep their RocksDB state intact.

### Changed

- `crates/stf/src/runtime.rs`: `openapi_spec()` now overrides the upstream Sovereign SDK `info` block with Ligate Chain identity. Title becomes `Ligate Chain JSON API`, contact becomes `Ligate Labs <hello@ligate.io>`, version pulls from `env!("CARGO_PKG_VERSION")` (so it tracks the workspace version instead of being hardcoded to `0.1.0`), license set to `Apache-2.0 OR MIT`. Description is DA-agnostic per the project's DA-isolation policy. Visible via `curl https://rpc.ligate.io/v1/openapi-v3.json | jq .info` and on the swagger UI page header. (#377)
- `scripts/backup-rocksdb.sh`: cold-snapshot path (daily + weekly tiers) now uses `systemctl stop --no-block` followed by SIGKILL escalation after a 3s grace window. Fixes a 5-min cold-backup downtime caused by two stacked bugs: (1) upstream Sovereign SDK shutdown hang where the process logs "shutdown complete" within ~1s but doesn't actually exit, eating `TimeoutStopSec=300`; (2) using bare `systemctl kill` triggered `Restart=on-failure` auto-restart, defeating the cold snapshot by re-starting the chain mid-rsync. Verified live: cold backup downtime dropped from 5.5 min to 32 s. (#375)
- `scripts/backup-rocksdb.sh`: manifest height capture now prefers the chain RPC (`/v1/ledger/slots/latest`) over the Prometheus metrics gauge (`ligate_block_height`). RPC returns the real head height as soon as the HTTP server is up; the metrics gauge takes ~2 min to populate after a cold-backup restart, which made the post-restore manifest record `"unknown"` even though the chain was healthy. (#375)
- `ops/backup/sudoers.d/ligate-backup`: extended NOPASSWD rule set to allow `systemctl stop --no-block`, `kill`, `kill --signal=SIGTERM`, `kill --signal=SIGKILL` against `ligate-node.service`. sudo matches by exact arg list, so each escalation variant needs its own rule. Same narrow service scope. (#375)
- Documentation prose: em dashes replaced with colons / semicolons / parens across CHANGELOG `[0.1.2]` section, `docs/development/public-devnet-deploy.md`, `docs/development/runbooks/backup-restore.md`, `monitoring/grafana/README.md`, `ops/backup/systemd/ligate-backup@.service`, and `scripts/restore-rocksdb.sh`. No semantic shifts. (#378)

### Fixed

- Swagger UI at `https://rpc.ligate.io/v1/swagger-ui/` previously rendered blank because the bundled `swagger-initializer.js` hardcodes its OpenAPI spec URL as `/openapi-v3.json` (without the `/v1` prefix the chain actually serves it under). Fixed operator-side with a Caddy `handle /openapi-v3.json { rewrite * /v1/openapi-v3.json; reverse_proxy 127.0.0.1:12346 }` block; the chain binary itself doesn't change. Caddyfile snippet codified in `docs/development/public-devnet-deploy.md`. (#376)
- `.github/workflows/release.yml`: changelog-excerpt step now extracts from the version-specific section (`## [X.Y.Z]`) that matches the tag being built, instead of `## [Unreleased]`. Our release hygiene folds `[Unreleased]` into the new versioned section BEFORE tagging (Keep-a-Changelog standard), which meant the workflow was reading an empty `[Unreleased]` block and silently shipping releases with one-byte bodies (v0.1.2 was the first visible casualty; backfilled manually after the fact). Fallback chain: versioned section → `[Unreleased]` → one-liner placeholder, so future releases never ship blank. (#379)

## [0.1.2] - 2026-05-17

Launch-eve hardening pass: chain ops infrastructure (backups, monitoring, persistent-disk migration, runbook fixes) plus the workspace version bump that makes the binary self-report its real version (was `0.0.1` regardless of git tag).

**Versioning convention change.** Dropping the `-devnet` suffix from release tags starting with this release. Past tags (`v0.1.0-devnet`, `v0.1.1-devnet`) stay as-is for archaeology, but going forward the convention is clean semver: `v0.1.2`, `v0.1.3`, ..., `v1.0.0-rc.1`, `v1.0.0` for mainnet GA. The `v0.x.y` numbering already signals pre-1.0 / unstable; `-devnet` was redundant AND semver-wrong (it's a pre-release identifier meaning "before 0.1.2 stable" but we were treating it as a final release). Network identity belongs in `chain_id` (`ligate-devnet-1`, future `ligate-testnet-1` / `ligate-mainnet-1`) and genesis directory names, not in the binary's semver. This matches how every mature chain does it (Solana, Aptos, Cosmos, etc.; only Sui prefixes tags with network name, and that's because they have divergent release trains per network, which we don't). Companion repos (`ligate-cli`, `ligate-js`, `ligate-api`) adopt the same convention at their next release.

No state-breaking changes; `chain_hash` unchanged. Operators upgrading from `v0.1.1-devnet` swap the binary in place; no re-genesis.

**State-preservation note.** `attestation.json` / genesis bytes / `chain_hash` all unchanged from `v0.1.1-devnet`. Operators deploying this binary to an existing `devnet-1` VM keep their RocksDB state intact across the swap.

### Added

- `monitoring/grafana/`: two production dashboards committed to repo as canonical source. `ligate-operator.json` (Prometheus-backed, 9 rows including the new Host VM row with CPU / RAM / state disk / root disk / load / uptime / network + disk-I/O timeseries) and `ligate-investor.json` (Infinity-backed business metrics). Re-import + export procedures documented in `monitoring/grafana/README.md`. (#361)
- `ops/backup/`: scheduled RocksDB snapshots to a private GCS bucket actually deployed end-to-end. Hourly tier (every 15 min, 7d retention via lifecycle), daily tier (03:30 UTC, 60d), weekly tier (Sun 04:00 UTC, 365d). systemd template unit (`ligate-backup@.service` with `Environment=TIER=%i`) so each timer fires under its own tier prefix in GCS. (#365, #366)
- `monitoring/grafana/ligate-operator.json` gains 6 host-VM stat panels powered by Alloy's `prometheus.exporter.unix` (CPU%, RAM%, persistent-disk fill %, boot-disk fill %, load1, uptime). (#364, #370)
- `ops/backup/sudoers.d/ligate-backup`: narrow drop-in granting the `ligate` user `NOPASSWD` for `systemctl stop|start ligate-node.service` only. Required for the cold-snapshot path used by daily + weekly tiers. (#372)
- `docs/development/public-devnet-deploy.md` gains a `--mode follower` operator step (the follower guard from #243/#248 was already in code; the runbook just didn't mention it). Same step adds the sudoers install for cold-backup-capable hosts. (#372)

### Changed

- `scripts/backup-rocksdb.sh`: daily + weekly tiers now run in **cold** mode (briefly stop ligate-node for a consistent rsync). Fixes a NOMT mid-write inconsistency that caused `nomt-1.0.3` to panic on restore (`assertion failed: pn.0 < max_pn` in the beatree free-list allocator) on snapshots taken while NOMT was writing `bbn`/`ht`/`ln`/`meta`/`wal`. Hourly stays hot/best-effort for fast rollback. Pre-stop height capture so the manifest records the snapshot's real height even on cold runs. (#372)
- `scripts/backup-rocksdb.sh`: `rsync -a --sparse` (was `-a` alone) preserves NOMT sparse-file holes on copy: each local snapshot now ~1.2 GB instead of ~5 GB. (#368)
- `scripts/backup-rocksdb.sh`: SIGPIPE bug in the manifest height-capture pipeline. `set -o pipefail` + `awk '… exit'` was making `curl` exit non-zero, which fired the `|| echo "unknown"` fallback AND captured awk's output, producing `"block_height": "15187\nunknown"` (literal newline in the JSON string value). Switched to a `<<<` here-string so awk reads from a temp file instead of a live pipe: no SIGPIPE possible regardless of when awk exits. (#368)
- `scripts/restore-rocksdb.sh`: handles both flat and double-nested GCS object layouts. `backup-rocksdb.sh` uploads via `gcloud storage cp -r SRC/ DST/`, which double-nests the timestamp (`<HOSTNAME>/<TIER>/<TS>/<TS>/...`). Restore now detects and flattens. Also re-derives `HOME` from the effective user so `gcloud` finds its config when the script runs via `sudo -u ligate` from another user's session. (#371)
- `docs/development/public-devnet-deploy.md`: VM image flipped from Debian 12 to Ubuntu 24.04 (the released `ligate-node` binary links GLIBC 2.39; Debian 12 has 2.36 and fails to load the binary). Celestia bumped to v0.30.2 (new `celestia-node_Linux_x86_64.tar.gz` artifact layout). VM sizing table updated to the actual prod shape. Adds an explicit "OS choice" note explaining why Ubuntu. (#371)
- `docs/development/public-devnet-deploy.md`: cloud-init pre-creates `/var/lib/ligate/rocksdb` so the operator can symlink the chain's storage path into it before first boot. Step 3 of the operator-still-has-to list gains the symlink command. Without this, the chain writes state to `/dev/root` on the 20 GB boot disk and a VM image rebuild loses chain history. (#369)
- `docs/development/runbooks/backup-restore.md`: full rewrite of the one-time setup section to reflect what actually works (VM scope bump for storage write, since `iam.disableServiceAccountKeyCreation` blocks the documented SA-JSON-key path). Adds in-place migration procedure (done on `ligate-devnet-1-sequencer` 2026-05-17, post-mortem informs the runbook for future hosts). Adds online persistent-disk resize procedure. Known-limitations table at the end documents remaining followups. (#365, #367, #368)
- `Cargo.toml`: workspace version bumped from `0.0.1` to `0.1.2-devnet`. Previously the workspace-default `0.0.1` propagated to every binary's self-reported version (including what `/v1/info` surfaces) regardless of the git tag. The "tagged release ceremony" pattern that justified the gap was internal-only convention; for the external `/v1/info` consumer the gap was confusing ("why does it report 0.0.1 when the tag is v0.1.1?"). Going forward, workspace version IS the release version and follows the tag.

### Fixed

- `ligate-node` systemd unit: `TimeoutStopSec=300` (was the systemd default of 90s). RocksDB compaction + WAL flush on shutdown can take 2-3 min on a hot state DB; the default triggered a SIGKILL during the `v0.1.1-devnet` swap on 2026-05-16. (#364)
- `/etc/alloy/config.alloy` on the chain VM: adds `prometheus.exporter.unix "node"` (in-process node_exporter) + matching `prometheus.scrape "node"`. 265 `node_*` series now flow to `grafanacloud-ligate-prom` under `job="integrations/unix"`, powering the new Host VM dashboard row. (#364)

## [0.1.1-devnet] - 2026-05-16

Pre-launch alignment release. Two operator-visible changes (genesis re-substitution onto the round-2 operator key, sig-verify error UX) and a handful of infrastructure-side tightening (SDK fork pin alignment with the api repo, release-workflow tagging behaviour, pre-commit gate, brand mark refresh).

**State-preservation note.** This release **does not change `attestation.json` fee config or any other genesis bytes that affect `chain_hash`.** Operators deploying this binary to an existing `devnet-1` VM keep their RocksDB state intact across the swap; no re-genesis required. Verified pre-release that `chain_hash` is unchanged from `v0.1.0-devnet`.

### Added

- `feat(attestation)`: signature-verification error path now surfaces both the canonical digest and the submitter address alongside the existing "signature invalid" message. Closes the debug-loop pain where attestation submitters got "sig invalid" with no way to cross-check which digest the chain re-computed versus what the off-chain signer produced. Pairs with a new `docs/protocol/attestation-v0.md` §wire-format appendix that documents the canonical LIP-5 test vector so client SDKs can byte-check their own digest computation against the reference. The `ligate-js` `v0.1.1-devnet` and `ligate-cli` `v0.1.2-devnet` releases both add cross-impl parity tests against the same vector. (#351)

### Changed

- Brand mark refresh on chain `README.md`: new three-piece glyph + the "Ligate Chain" lockup, matching the marketing-side rebrand (R3). All-sage palette, no opacity drop on subsidiary text. (#341)
- `.github/workflows/release.yml`: only tags matching `v*-rc.*` are flagged as GitHub-side prereleases now; bare `vX.Y.Z-devnet` ships as a normal release. Closes the bug where every `-devnet` tag was getting tagged "Pre-release" in the GitHub UI, which confused operators expecting `-devnet` to be the canonical devnet artifact. (#342)
- `devnet-1/genesis/` re-substituted with the round-2 operator key. The round-1 substitution (PR #251, 2026-05-08) replaced the public-seed bootstrap placeholder with `lig1rh9vcu879l36...`, but the private key for that address was never persisted to `~/.ligate-keys/` and is unrecoverable. No public-facing system ever read the round-1 `chain_hash` (devnet never came up against it), so this round-2 substitution has zero downstream blast radius -- partners haven't seen the round-1 hash anywhere. Round 2 also substitutes the all-zeros Celestia DA placeholder (`celestia1qqqqqq...zf30as`) with the real operator-controlled Mocha wallet (`celestia1mphanjz...`), which round 1 left untouched. The operator key is now persisted in three places to prevent recurrence: `~/.ligate-keys/devnet-1/operator.key` (chmod 600), `~/.config/ligate/secrets.env` as `LIGATE_DEVNET1_OPERATOR_KEY`, and GCP Secret Manager (`projects/utopian-spring-494915-q1/secrets/ligate-devnet-1-operator-key`). Closes the broken-key issue surfaced during the GCP devnet bring-up. (#343)
- `.pre-commit-config.yaml` added. Local `cargo fmt --check` runs at commit time so the same gate CI uses fires before the commit leaves the developer machine, instead of after a CI roundtrip. One-time setup: `pre-commit install`. Matches the `ligate-api` and `ligate-cli` repos. (#353)

### Deps

- SDK fork pin bumped to `ligate-io/sovereign-sdk@eab3f9d0` (was `49e9b2057`). Picks up the upstream `NodeClient::get_nonce_for_public_key` uniqueness-path fix. Realigns the [patch] table with `ligate-api` and `ligate-cli`, which both bumped to the same revision. Chain runtime behaviour unchanged (the fix is client-side, affecting how SDK-mediated callers fetch nonces — the chain itself isn't a NodeClient consumer in its hot path). (#352)

## [0.1.0-devnet] - 2026-05-15

### Changed

- `docs/development/public-devnet-deploy.md` updated to the chain-only-on-GCP architecture: VM sizing dropped from `e2-standard-4` (4 vCPU / 16 GB / $100) to `e2-medium` (2 vCPU / 4 GB / $25) since the chain VM no longer co-hosts the faucet, indexer, or explorer. Step 4.5 (canonical-schema ceremony) clarified to run from a workstation rather than the VM (operator key stays off the production host). Step 4.7 (faucet deploy) replaced with a pointer at [`ligate-io/ligate-api`](https://github.com/ligate-io/ligate-api), the new unified Rust HTTP service that hosts drip + indexer endpoints together on Railway, alongside a Railway-managed Postgres. The split keeps the GCP VM minimal and lets api/Postgres scale independently from the chain. Frontend split to `ligate-io/ligate-explorer` (Next.js on Vercel). Closes the architecture refactor tracked in `ligate-io/ligate-api`'s scaffold commit.

### Added

- `crates/bootstrap-cli/examples/disc_probe.rs` — one-shot fixture probe that prints byte-exact `borsh(RuntimeCall::Attestation(...))` output for `RegisterAttestorSet` / `RegisterSchema` / `SubmitAttestation`, plus the deterministic `AttestorSet::derive_id` / `Schema::derive_id` outputs for known-input fixtures. Used as the source-of-truth for the `@ligate/sdk` TS-side wire-format pinning (`ligate-js/test/attestation.test.ts`); regenerate fixtures after any Sovereign SDK pin bump that touches runtime composition or `attestation::CallMessage` shape. `cargo run --example disc_probe -p ligate-bootstrap-cli`. No CI changes — this is a developer tool, not a runtime gate.

### Changed

- `docs/development/public-devnet-deploy.md` gains two new operator-flow steps: **Step 4.5** (run the canonical-schema ceremony from PR #249's `ligate-bootstrap` binary; the chain ships with `initial_attestor_sets: []` / `initial_schemas: []` so first-party schemas register post-genesis via on-chain tx) and **Step 4.7** (deploy the faucet on the same VM as `ligate-node`: install / fund-from-operator / systemd unit / env-var setup / smoke test). Both blocks lift the canonical commands operators run on Day 1, so the runbook is now end-to-end actionable from "GCP VM exists" to "partners can drip and use the chain". No code changes; documentation only.
- `devnet-1/genesis/` substituted with operator-controlled keys for `ligate-devnet-1`. The placeholder bootstrap address (`lig1h72nh5c7jfjkcygku4thsh2t53dyh33kkpktpy84w06qwr4agvt`, derivable from a public seed) is replaced with the real operator key (`lig1rh9vcu879l36z42xgw78wuksy8ma5vnzkrregmgmka9uqr8fyp8`) across all 6 module configs that reference it (sequencer registry, attester / prover / operator incentives, bank treasury + admins, attestation treasury). Two demo wallets (`lig19tzz...`, `lig1e0lp...`) ship pre-funded with 50M LGT each for partner-integration testing. v0 uses the single-operator-key model: one address fills bootstrap + sequencer + treasury + reward; security-separated treasury / sequencer keys land at testnet (#231 follow-up). Private keys live offline at `~/.ligate-keys/devnet-1/<role>.key` (chmod 600, never in repo). The `chain_hash` is now finalised for `ligate-devnet-1` — anything signed against the prior placeholder hash will fail signature verification on the public network. Closes [#231](https://github.com/ligate-io/ligate-chain/issues/231).

### Added

- `crates/rollup/tests/localnet_config.rs` gains `devnet_1_does_not_ship_a_rollup_toml` — a one-line file-inventory guard for the Celestia-only-by-design `devnet-1/` directory (`devnet-1/README.md`). If a future PR copy-pastes `devnet/rollup.toml` into `devnet-1/` by mistake, the test breaks loudly at PR time rather than silently letting an operator boot the public-devnet config in MockDA mode. Companion to the existing chain-id pin in `devnet_1_celestia_toml_parses_against_celestia_blueprint_types`. Closes [#190](https://github.com/ligate-io/ligate-chain/issues/190) (the public-devnet config drift coverage).
- `ligate-bootstrap-cli` operator binary for post-genesis canonical-schema registration. Two subcommands: `compute-shape-hash <path>` (pure I/O + SHA-256, prints the `payload_shape_hash` for a JSON Schema doc; useful for verifying a checked-in schema's hash without booting the chain) and `register-canonical-schemas --config <toml> --signer-key <file> --rpc <url>` (reads a `canonical-schemas.toml`, hashes each referenced spec doc, queries the chain for already-registered ids, and submits one `RegisterAttestorSet` + one `RegisterSchema` tx per `[[schemas]]` block, polling `/v1/ledger/txs/{hash}` for inclusion). Idempotent re-runs skip already-landed registrations. The binary mirrors the cli's HTTP-poll wait pattern (cli #9 / faucet #5 / chain #245), auto-appends `/v1` to RPC URLs idempotently, and fetches `chain_hash` from `/v1/rollup/info` on startup so operators only need to pass `--rpc` and `--signer-key`. Lives at `crates/bootstrap-cli/`. Companion `docs/development/canonical-schema-registration.md` documents the operator runbook for the post-boot ceremony, expected outputs, verification commands, troubleshooting table, and how to add a future first-party schema (Mneme, Iris, Kleidon products) to the same registration flow. 16 unit tests cover the canonical-form check (BOM / CRLF / trailing-newline detection on the schema docs), TOML config parsing + validation (threshold bounds, hex-shaped pubkeys, fee-route consistency), and the `/v1` URL normaliser.
- `docs/protocol/schemas/themisra.proof-of-prompt-v1.json` — first canonical Themisra schema, v1 of LIP-5 (#185). Required fields: `spec_version` (pinned to `1.0.0`), `user_did` (chain-native `lig1...`, W3C DID URL, or `did:lig:<address>`), `model_id` (`<provider>/<model>:<version>` recommended), `prompt_hash` + `response_hash` (64-char hex SHA-256, NFC-normalized + LF + UTF-8 canonicalization), `created_at` (RFC 3339 UTC). Optional: `system_prompt_hash`, `provider_id`, `tool_id`, `prompt_uri`, `response_uri`. Full JSON Schema 2020-12 doc with `additionalProperties: false`, length / pattern bounds on every field. The off-chain attestor quorum service validates payloads against this spec; the chain stores only the SHA-256 of the file's bytes (`payload_shape_hash` = `9ac4625714b18111f53237f7906ad3bba1e8d2a584db5afd8cc3f823cbed1bdf`). Spec status is "v1 sufficient for devnet"; LIP-5 may iterate, but iterations ship as v2+ with their own `schema_id`, never as mutations of v1. Companion `docs/protocol/schemas/README.md` documents the canonicalization rules (no BOM, LF-only, single trailing newline, `prettier` enforces) and how the `payload_shape_hash` contract works.
- `devnet-1/canonical-schemas.toml.example` template documenting the post-boot registration ceremony for `themisra.proof-of-prompt v1` (and a slot for future schemas). Operator copies to `canonical-schemas.toml` (gitignored), substitutes the placeholder federated-quorum pubkeys with the real ones, runs `ligate-bootstrap register-canonical-schemas`. The 2-of-3 quorum threshold is the v0 default; production may grow to 3-of-5 as more design partners run attestor nodes. Refs [#231](https://github.com/ligate-io/ligate-chain/issues/231).
- `ligate-client` gains a feature-gated `submit` module wrapping the Sovereign SDK's `NodeClient` for the operator-facing build-sign-submit-wait pipeline. New `Submitter` struct exposes `submit_raw_tx` (single tx) + `submit_raw_txs` (batch), with optional `wait_for_inclusion` polling. Default builds remain pure typed-message construction (no tokio / reqwest pulled in for in-zkVM guest or pure-builder consumers); the `submit` feature opts into the network-side pieces. Module-level docs walk through the 5-step build-sign-encode-submit-poll wire format. Companion `crates/client-rs/README.md` covers both default + submit-feature usage with worked examples and links to the faucet repo's signer.rs as a full integration reference. Closes [#240](https://github.com/ligate-io/ligate-chain/issues/240).
- Starter Grafana dashboard at `ops/grafana/ligate-node.json`. Three rows (chain activity, node health, errors / rejections), eight panels covering all Phase 1 metrics: `ligate_schemas_registered_total`, `ligate_attestor_sets_registered_total`, `ligate_attestations_submitted_total` (rate), `ligate_block_height`, `ligate_state_db_size_bytes`, `ligate_rpc_requests_total{endpoint}` (rate), `ligate_attestations_rejected_total{reason}` (top-10 stacked bars over 1h), `ligate_rpc_request_duration_seconds_bucket` (p95 by endpoint). Companion `ops/grafana/README.md` walks through the import flow + a reference Prometheus scrape config. `docs/development/devnet.md` "Phase 1 / Phase 2 metrics" section now links the dashboard. Closes [#165](https://github.com/ligate-io/ligate-chain/issues/165).
- `ligate-genesis-tool keys generate` subcommand. Stand-in for the eventual `ligate-cli keys generate` ([#112](https://github.com/ligate-io/ligate-chain/issues/112)) until that ships in its own repo. Takes `--roles a,b,c --output DIR` and produces one Ed25519 keypair per role: `<role>.key` (64-char hex, chmod 600, never commit) and `<role>.address` (`lig1...` bech32m). Address derivation matches the SDK's standard pattern: `Address = SHA-256(pubkey)[..28]`. Stdout emits a `devnet-1/keys.toml` stub the operator pastes into the substitution map; stderr carries the human-friendly per-role summary. Unblocks step 1 of the operator workflow ([#231](https://github.com/ligate-io/ligate-chain/issues/231)). Three new unit tests cover address shape, independence, and role-as-filename-stem.
- `devnet-1/keys.toml.example` template for genesis substitution. Documents every placeholder address shipping in `devnet-1/genesis/` (the bootstrap-everything fixture, two demo wallets), maps each to the runtime roles it fills (sequencer registry / treasury / reward / attester / prover bond), and walks through the `ligate-genesis-tool` flow for substituting in operator-controlled keys. Companion `devnet-1/.gitignore` ensures the filled-in `keys.toml` (operator-specific, not committed) doesn't leak into the repo. `devnet-1/README.md` "Placeholder addresses" section rewritten with the concrete five-step bring-up flow (generate keys offline → fill `keys.toml` → `genesis-tool generate` → `verify` → ceremony PR), plus a callout that the Celestia hot key (`SOV_CELESTIA_SIGNER_KEY`) is separate from the genesis substitution and sourced via the Mocha faucet. Refs [#231](https://github.com/ligate-io/ligate-chain/issues/231).
- `codecov.yml` configuration. Sets the project + patch coverage target to 70% (down from codecov's default 80%) to match pre-mainnet maturity, with a 2% threshold so small fluctuations don't trigger warnings. Targets will tighten as the project matures: 70% pre-mainnet, 80% at testnet, 85% at mainnet. Comment behaviour set to `require_changes: true` so docs-only / CI-only PRs don't get spammed with no-op coverage notes. Coverage remains informational only — the `coverage` CI job is still NOT in `ci-pass`'s `needs` list.
- LIPs (Ligate Improvement Proposals) scaffold at `docs/protocol/lips/`. Modelled on Ethereum's EIPs, Celestia's CIPs, and Bitcoin's BIPs. `README.md` documents the lifecycle (`Draft → Review → Last Call → Final`, with `Withdrawn` / `Rejected` / `Stagnant` off-ramps), the numbering convention, file format with YAML frontmatter (Status, Type, Category, Created, etc.), and the trigger conditions for needing a LIP (new modules, wire-format changes, consensus changes, first-party schemas, anything that bumps the chain-id ladder). `lip-template.md` provides the canonical body structure. `lip-1.md` is the meta-LIP covering the process itself. LIP-2 through LIP-5 reserved for the existing RFC tracking issues ([#127](https://github.com/ligate-io/ligate-chain/issues/127) PoUA, [#183](https://github.com/ligate-io/ligate-chain/issues/183) receipt embedding spec, [#184](https://github.com/ligate-io/ligate-chain/issues/184) content-provenance schema, [#185](https://github.com/ligate-io/ligate-chain/issues/185) prompt-licensing + content-licensing schemas) which will be drafted as their authors begin. Sibling tracking issue: split LIPs to `ligate-io/lips` repo with mdBook rendering once ≥3 LIPs reach `Final`. Closes [#211](https://github.com/ligate-io/ligate-chain/issues/211).
- `docs/audits/` directory with a `README.md` documenting the convention for filing external audit reports: paired `<YYYY-MM>-<auditor>-<scope>.pdf` + `<YYYY-MM>-<auditor>-<scope>.md` files, sidecar markdown shape with findings table + remediation tracking, severity / status taxonomy, CHANGELOG cross-link pattern, and the audit-cadence plan (pre-`ligate-testnet-1` scoped audit, pre-`ligate-1` full-protocol audit, post-mainnet upgrade-module ceremony audits). Empty today by design; first audit lands closer to mainnet. Main README gains a "Security" section linking here plus the existing in-repo security signal (cargo audit, cargo deny, Borsh snapshots, fuzz, DA-isolation, threat model). Closes [#208](https://github.com/ligate-io/ligate-chain/issues/208).
- Codecov line-coverage integration. New `coverage` job in `.github/workflows/ci.yml` runs `cargo llvm-cov --workspace --lib --tests --lcov` and uploads to codecov.io via `codecov/codecov-action@v5` (OIDC, no token needed for public repos). Coverage is informational only; the job is intentionally NOT in `ci-pass`'s `needs` list, so codecov outages or transient upload failures never block a merge. Doctests skipped because cargo-llvm-cov's `--doctests` requires nightly. Codecov badge added to README between CI and Latest release. Closes [#219](https://github.com/ligate-io/ligate-chain/issues/219).
- README badge row gains "Latest release" (links to GitHub Releases page, includes pre-releases) and "Ask DeepWiki" (`deepwiki.com/ligate-io/ligate-chain`) badges. Standardised order: CI, Release, License, Docs, DeepWiki, Pre-devnet status. Closes [#207](https://github.com/ligate-io/ligate-chain/issues/207).
- `examples/docker-compose.yaml` orchestrating `ligate-node` + a co-located Celestia Mocha light node as separate containers. Three-command quickstart for follower operators (`git clone`, `cp .env.example .env`, `docker-compose up -d`). Companion `examples/README.md` covers follower vs sequencer mode, the `chain_hash` cross-check verification flow, and the genesis-substitution path via `ligate-genesis-tool`. Deployment runbook updated with a "Docker Compose variant" section. Closes [#221](https://github.com/ligate-io/ligate-chain/issues/221).
- Docker image distribution for `ligate-node`. Multi-stage `Dockerfile` (rust:1.93-bookworm builder → debian:bookworm-slim runtime, ~50 MB final image, unprivileged `ligate` user) plus `.github/workflows/docker.yml` that builds for `linux/amd64` and `linux/arm64` via buildx and pushes to GHCR (`ghcr.io/ligate-io/ligate-chain:VERSION`, plus `:latest` for full releases). Shares the GitHub Actions build cache across runs so subsequent tags ship in minutes rather than rebuilds. `workflow_dispatch` triggers a multi-arch build without pushing, for dry-run validation. Closes [#195](https://github.com/ligate-io/ligate-chain/issues/195).
- GitHub Releases workflow at `.github/workflows/release.yml`. Triggers on `v*` tag push. Cross-compiles `ligate-node` natively on three runners (`ubuntu-24.04` for `x86_64-unknown-linux-gnu`, `ubuntu-24.04-arm` for `aarch64-unknown-linux-gnu`, `macos-14` for `aarch64-apple-darwin`), strips symbols, packages each as `ligate-node-${VERSION}-${SUFFIX}.tar.gz` plus a `.sha256` checksum, includes the LICENSE files. Final job creates a GitHub Release with all artifacts attached and the `## [Unreleased]` section of `CHANGELOG.md` as release notes. Tags containing a hyphen (`-rc.N`, `-devnet`, etc.) auto-flag as pre-releases. `workflow_dispatch` runs the matrix without publishing for dry-run validation. Closes [#194](https://github.com/ligate-io/ligate-chain/issues/194).
- `ligate-genesis-tool` operator binary with `verify` and `generate` subcommands. `verify` re-runs the cross-module genesis validator against a directory of JSONs without booting the chain, surfacing the typed `GenesisError` variants from #175 plus the I/O / parse errors from `create_genesis_config`. `generate` substitutes addresses (and optionally token balances) in a template bundle per a TOML substitution map, then runs `verify` on the output before returning. Operator iteration loop on a hand-edited bundle drops from "boot the chain to find a typo" to "edit JSON, run verify, read error, repeat". 8 tests total (4 unit on the substitution + balance-override walk, 4 integration via child-process invocation against the localnet bundle plus a deliberately corrupted control). Closes [#191](https://github.com/ligate-io/ligate-chain/issues/191).
- `docs/development/public-devnet-deploy.md` operator runbook for both sequencer and follower roles on GCP, covering VM sizing, `cloud-init` scaffold, Caddy reverse-proxy setup, Prometheus scrape pattern, GCS-backed backup cron, the `chain_hash`-cross-check verification flow, an 8-row failure-mode triage table, and the Mocha-reset re-genesis playbook. Closes [#189](https://github.com/ligate-io/ligate-chain/issues/189).
- `devnet-1/` public-devnet config artifacts: `celestia.toml` with `chain_id = "ligate-devnet-1"` plus a full `genesis/` bundle mirroring the localnet shape, with operator-runbook narrative in `devnet-1/README.md` covering the placeholder-address pattern, env-var override surface, and `ligate-genesis-tool` substitution path before public deploy. Closes [#188](https://github.com/ligate-io/ligate-chain/issues/188). CI drift coverage extending `crates/rollup/tests/localnet_config.rs` with two new tests pinning `chain.chain_id == "ligate-devnet-1"` on the public-devnet TOML and running cross-module genesis validation on the public-devnet JSONs; closes [#190](https://github.com/ligate-io/ligate-chain/issues/190).
- Doctests on the public protocol surface. 13 new runnable examples across the `attestation` crate (`Schema::derive_id`, `AttestorSet::derive_id`, `AttestationId::from_pair`, `SignedAttestationPayload::digest`, all three `CallMessage` variants, the `Schema` and `AttestorSet` structs, plus a crate-level end-to-end build flow) and the `ligate-client` crate (`register_attestor_set`, `register_schema`, `build_signed_submit`). `cargo test --workspace --doc` goes from 1 to 14 passing doctests; closes [#96](https://github.com/ligate-io/ligate-chain/issues/96).
- `chain_id` config field with ladder validation. The rollup TOML's new `[chain]` section carries the canonical Cosmos-style identifier (`ligate-localnet`, `ligate-devnet-N`, `ligate-testnet-N`, `ligate-N`); the binary refuses to boot with any value outside that ladder. New `GET /v1/rollup/info` REST endpoint returns the configured `chain_id`, the runtime's build-time `CHAIN_HASH` (hex), and the binary version, so wallets and explorers can identify the network without out-of-band knowledge. The rollup config loader does a two-pass split (`[chain]` extracted, residual handed to the SDK's `RollupConfig`) since the SDK's `deny_unknown_fields` precludes mixing the two structs in one type. Closes [#181](https://github.com/ligate-io/ligate-chain/issues/181).
- `Schema::payload_shape_hash`: 32-byte content-address of the off-chain payload spec, threaded through `Schema`, `InitialSchema`, `CallMessage::RegisterSchema`, the `register_schema` client builder, and `seed_from_config`. Chain stores the value verbatim and never verifies it; `[0u8; 32]` is the documented opt-out. Borsh wire-format snapshots updated for the new layout, three integration tests pin the storage round-trip via runtime tx, runtime tx with the opt-out value, and `InitialSchema` at genesis. Spec section in `docs/protocol/attestation-v0.md` documents the field's chain-stores-not-verifies stance and the multi-SDK drift-protection rationale ([#151](https://github.com/ligate-io/ligate-chain/issues/151)).
- Prometheus `/metrics` endpoint on `ligate-node` with five core metrics: schema / attestor-set / attestation success counters, `attestations_rejected_total{reason}` labelled by `AttestationError` discriminant, `state_db_size_bytes` gauge (on-disk allocation), `block_height` gauge polled from `LedgerStateProvider`, and `rpc_requests_total` + `rpc_request_duration_seconds` histograms via axum middleware. Process metrics auto-ship on Linux. Tracking issue [#110](https://github.com/ligate-io/ligate-chain/issues/110), Phase 1 in [#163](https://github.com/ligate-io/ligate-chain/pull/163), Phase 2 across [#167](https://github.com/ligate-io/ligate-chain/pull/167) [#168](https://github.com/ligate-io/ligate-chain/pull/168) [#169](https://github.com/ligate-io/ligate-chain/pull/169) [#170](https://github.com/ligate-io/ligate-chain/pull/170).
- `AttestationError::discriminant()` mapping all 18 variants to stable snake_case labels for the rejection counter, pinned by a unit test ([#167](https://github.com/ligate-io/ligate-chain/pull/167)).
- `cargo-deny` supply-chain gate enforcing license allow-list, source-allow-list, and duplicate version policy alongside the existing `cargo audit` ([#154](https://github.com/ligate-io/ligate-chain/pull/154), closes [#120](https://github.com/ligate-io/ligate-chain/issues/120)).
- `cargo-fuzz` harnesses for Borsh decoders (`CallMessage`, `Schema`, `AttestorSet`, `Attestation`, `SignedAttestationPayload`) plus Bech32m parsing and `AttestationId` round-trip. Nightly cron workflow with ~10 minute time budget. ([#156](https://github.com/ligate-io/ligate-chain/pull/156), closes [#119](https://github.com/ligate-io/ligate-chain/issues/119)).
- `.github/CODEOWNERS` with per-path review routing to `@ligate-io/maintainers` and `@ligate-io/protocol-reviewers` ([#173](https://github.com/ligate-io/ligate-chain/pull/173), closes [#113](https://github.com/ligate-io/ligate-chain/issues/113)).
- `docs/protocol/da-layers.md` documenting the DA-agnostic-by-construction architecture, surface inventory, and adapter playbook for adding a new DA backend ([#159](https://github.com/ligate-io/ligate-chain/pull/159), closes [#117](https://github.com/ligate-io/ligate-chain/issues/117)).
- `docs/protocol/threat-model.md` covering trust posture across the chain-id ladder, attestation-layer adversaries, sequencer-layer threats, wire-format runtime risks, cryptographic primitive failure, operational adversaries, and protection inventory ([#148](https://github.com/ligate-io/ligate-chain/pull/148), closes [#121](https://github.com/ligate-io/ligate-chain/issues/121)).
- `docs/protocol/rest-api.md` reference for all 115 auto-mounted REST endpoints with curl examples and Mermaid path map ([#147](https://github.com/ligate-io/ligate-chain/pull/147)).
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

- Release workflow (`.github/workflows/release.yml`) gains a fourth target: `x86_64-apple-darwin` (Intel Mac, archive suffix `darwin-amd64`) on the `macos-13` runner. Tagging a `v*` release now produces 4 tarballs + 4 SHA-256s. Few operators run Intel Macs as servers, but contributors on 2018-2019 hardware get a downloadable binary instead of a from-source build. Deploy runbook updated to mention all four targets. Closes [#210](https://github.com/ligate-io/ligate-chain/issues/210).
- `CONTRIBUTING.md` "Commit conventions" section gains a documented set of [Conventional Commits](https://www.conventionalcommits.org/) prefixes (`feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`, `ci:`, `deps:`) plus a sample commit shape. Soft convention, not CI-enforced; reviewers nudge but don't block. Existing freeform commit history stays as-is. Closes [#209](https://github.com/ligate-io/ligate-chain/issues/209).
- On-chain `bank.gas_token_config.token_name` flips from `"$LGT"` to `"LGT"` in both `devnet/genesis/bank.json` and `devnet-1/genesis/bank.json`. Matches industry convention (Cosmos, Solana, Ethereum / ERC-20 all store bare ticker on-chain), unblocks copy-paste of CLI examples that previously got bitten by `$LGT` shell expansion, and lets wallets render `1.5 LGT` instead of `1.5 $LGT` (the awkward double prefix). The `$LGT` form stays as the brand surface in marketing prose, doc comments, and external-facing copy. The on-chain `lgt_token_id` is unchanged because it's independently configured via `GAS_TOKEN_ID` in `constants.toml`, not derived from `token_name`. Closes [#198](https://github.com/ligate-io/ligate-chain/issues/198).
- Chain REST API now mounts under a `/v1/` prefix. All 114 auto-discovered endpoints move from `/...` to `/v1/...` so future breaking schema changes can land at `/v2/...` without colliding with existing clients. Operator probes (`/health`, `/ready`) and Prometheus `/metrics` (separate port) stay unversioned by convention. Pre-public-devnet timing means no client migration cost. Closes [#149](https://github.com/ligate-io/ligate-chain/issues/149).
- `attestation::max_builder_bps` migrated from a compile-time const to runtime state, governance-tunable per [#40](https://github.com/ligate-io/ligate-chain/issues/40)'s constants-to-state path. Default still `5000` (50%) for genesis-omitting configs. ([#152](https://github.com/ligate-io/ligate-chain/pull/152)).
- Devnet runbook (`docs/development/devnet.md`) refreshed to reflect post-Phase-A reality: actual binary, real config flags, current limitations table ([#107](https://github.com/ligate-io/ligate-chain/pull/107), closes [#84](https://github.com/ligate-io/ligate-chain/issues/84)).
- README Labs wordmark goes fully sage in the brand lockup, drops the 0.65-opacity treatment ([#106](https://github.com/ligate-io/ligate-chain/pull/106)).
- CI path-filter excludes `.github/dependabot.yml` so dependabot config edits don't trigger code-checking jobs ([#150](https://github.com/ligate-io/ligate-chain/pull/150)).
- Dependabot config ignores the schemars major and strum minor bumps that the SDK pin holds back ([#102](https://github.com/ligate-io/ligate-chain/pull/102)).

### Fixed

- CI's docs-only path-filter skip. Negative pattern `'!**.md'` was failing to exclude root-level markdown files (`README.md`, `CHANGELOG.md`) from the `code` filter, so docs-only PRs were running the full Rust gate suite (cargo check / clippy / test / doc / cargo-deny / coverage) anyway. picomatch's `'**.md'` doesn't reliably match path-without-directory, and `'**/*.md'` requires at least one directory segment, so neither alone covers both cases. Fixed by combining `'!*.md'` (root) with `'!**/*.md'` (nested) in `.github/workflows/ci.yml`. Docs-only PRs now correctly skip the heavy gates and `CI pass` accepts the skipped state, dropping doc PR turnaround from 5+ minutes to 30 seconds.
- README "Latest release" badge. shields.io's `/github/v/release/` endpoint returns `invalid` for our repo regardless of query-param shape (`include_prereleases`, `include_prereleases=true`, no params); the endpoint appears to be broken or rate-limited for prerelease-only repos. Switched to `/github/v/tag/?sort=semver` which reads directly from git tags and renders the latest semver tag (currently `v0.0.1-rc.4`). Same visual outcome, no `invalid` state.
- Codecov upload to protected `main` branch. Earlier integration ([#219](https://github.com/ligate-io/ligate-chain/issues/219)) assumed tokenless OIDC upload would work for public repos, but codecov rejects pushes to protected branches with `Token required because branch is protected`. Workflow now passes `token: ${{ secrets.CODECOV_TOKEN }}` (repo secret, not committed). PR-run uploads were unaffected; only main-branch uploads were dropped, so the badge stayed at "unknown" until this fix.
- `ligate_state_db_size_bytes` gauge now reports actual on-disk allocation via `meta.blocks() * 512` instead of nominal logical size via `meta.len()`. Caught during Tier 1 manual verification: gauge was reading 61 GB on a node with 23 MB actual disk usage because of NOMT and RocksDB sparse-file preallocation ([#171](https://github.com/ligate-io/ligate-chain/pull/171)).

### Security

- Audit ignore additions for transitive `astral-tokio-tar` advisories `RUSTSEC-2026-0112` and `RUSTSEC-2026-0113`, both reachable only through `sov-test-utils`'s `testcontainers` dev-dep, no production exposure ([#104](https://github.com/ligate-io/ligate-chain/pull/104)).

### Deps

- `actions/upload-artifact` bumped from v4 to v7 (dependabot, [#161](https://github.com/ligate-io/ligate-chain/pull/161)).
- `rustls` bumped from 0.23.39 to 0.23.40 (dependabot, [#162](https://github.com/ligate-io/ligate-chain/pull/162)).

[Unreleased]: https://github.com/ligate-io/ligate-chain/compare/main...HEAD
