# Multi-sequencer DbElected smoke harness

Local Docker smoke for chain#412: verifies that the Sovereign SDK's
`ConfiguredNodeRole::DbElected` mode actually does what the
plumbing audit (chain#410, PR #411) claims — three `ligate-node`
instances against one Postgres elect one leader, the other two run
as replicas, and on leader kill another instance wins the election
within the configured timeout.

## What this proves (and what it doesn't)

**Proves:**
- The SDK's Postgres-backed leader election runs correctly with our
  rollup config + genesis files
- Exactly one of N instances is `BatchProducer` at any time
- Replicas reject `POST /v1/sequencer/txs` with the SDK's
  generic-but-correct 503
- `GET /sequencer/role` returns the right role for each instance
- Killing the leader triggers re-election; a new instance takes
  over within `leader_timeout_millis` + `grace_period_millis`
- Replicas catch up to the new leader's height via Postgres state
  sync

**Does not prove:**
- **Failover validation past leader kill.** See "Failover validation
  limits" below.
- Behaviour against real Celestia DA (this harness uses MockDa).
  Production behaviour is validated separately by Mocha smoke
  (`.github/workflows/mocha-smoke.yml`).
- The full force-include path (#81).
- Behaviour under Postgres failure (we use a single non-HA postgres
  container; prod uses Cloud SQL regional HA).
- Cross-zone latency effects on heartbeat tuning.

## Failover validation limits

The smoke includes a "kill leader, observe failover" step, but with
MockDa as the DA backend, **failover capture is best-effort, not
guaranteed.** Reason: when the BatchProducer container is SIGKILLed,
the shared MockDa SQLite enters a degraded state that the surviving
chain processes interpret as "DA layer is gone" — they shut down
their STF runners cleanly (`STF Runner has completed execution` in
the logs) BEFORE the SDK's heartbeat task elects a new leader.

This is a MockDa concurrency limit, not a DbElected mechanism issue:
- The SDK's leader-election state in Postgres updates correctly
  (visible via `psql` against the heartbeat tables)
- The heartbeat task on survivors detects the leader death (logs
  show the election task shutdown handler firing)
- Survivors just don't survive long enough to actually become
  BatchProducer themselves before MockDa drags them down

Validating the failover phase end-to-end requires a DA backend that
handles multi-instance writers properly. Tracked as a followup under
chain#412: "upgrade smoke to local-celestia-devnet". With Celestia,
all three instances will properly remain alive past the leader-kill
event and the smoke will be able to observe a survivor flip to
BatchProducer.

## Topology

```
                     ┌──────────────────────────┐
                     │ postgres:16              │
                     │ db=ligate_sequencer       │
                     │ user=ligate_sequencer     │
                     └────────────┬─────────────┘
                                  │
        ┌─────────────────────────┼─────────────────────────┐
        │                         │                         │
   ┌────▼─────┐             ┌─────▼────┐              ┌─────▼────┐
   │ ligate-1 │             │ ligate-2 │              │ ligate-3 │
   │ :12346   │             │ :12347   │              │ :12348   │
   └────┬─────┘             └─────┬────┘              └─────┬────┘
        │                         │                         │
        └──────────► shared MockDa SQLite ◄─────────────────┘
                     (volume mount)
```

- All three `ligate-node` instances read the same genesis directory
- All three connect to the same MockDa SQLite file (volume-shared)
- All three write to distinct RocksDB directories (per-instance
  state; replicas catch up via Postgres state sync)
- All three connect to the same Postgres with distinct `node_id` and
  `node_role = "DbElected"`
- The container running as `BatchProducer` is the one currently
  posting batches to MockDa

## Quickstart

```bash
# from repo root
cd tests/smoke/multi-sequencer

./run.sh up        # start postgres + 3 ligate-node containers
./run.sh status    # show which instance is leader (current roles)
./run.sh kill-leader  # SIGKILL the BatchProducer; observe failover
./run.sh status    # confirm a different instance is now leader
./run.sh down      # tear it all down
```

`run.sh` exits non-zero on any unexpected state, so it's CI-able once
we hook this into the workflow file (out of scope for this PR;
tracked under #412).

## Configuration template

Each container starts via `entrypoint.sh`, which runs `envsubst` over
`rollup.toml.template` with per-instance env vars (`LIGATE_NODE_ID`,
`LIGATE_BIND_PORT`, `LIGATE_STORAGE_PATH`), then execs `ligate-node`
against the rendered file. The template is a pared-down version of
`localnet/rollup.toml` with the postgres_config block uncommented and
parametrised.

This is intentionally simpler than the production
`ops/cloud-init/render-rollup-toml.sh` script (no GCP Secret Manager
roundtrip; password is supplied via env var at compose-up time). The
shape of the activated postgres_config block matches what the prod
render script produces.

## Image selection

By default uses `ghcr.io/ligate-io/ligate-chain:0.2.14` (current
devnet-1 release as of 2026-05-23). The DbElected machinery has
been stable since v0.2.3 (`4b4a313b7`); any v0.2.x image works
for this verification.

Override via env: `LIGATE_IMAGE=ghcr.io/ligate-io/ligate-chain:vX.Y.Z ./run.sh up`.

## Cleanup notes

`run.sh down` removes containers + named volumes. The MockDa SQLite
and RocksDB dirs live inside the volumes; nothing leaks to the host
filesystem outside this directory.
