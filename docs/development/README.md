# Development docs

Operator + developer documentation for `ligate-chain`. This is the local-machine + GCP-VM-side surface; product-level docs (Themisra, Iris, Kleidon) live in their own repos, and the chain's protocol spec lives one level up at [`docs/protocol/attestation-v0.md`](../protocol/attestation-v0.md).

Layout:

- **Top-level guides** below — read-once orientation, deploy planning, design notes.
- **Runbooks** at [`runbooks/`](./runbooks/) — actionable, step-by-step procedures the operator runs against a real chain.
- **Alert runbooks** at [`runbooks/alerts/`](./runbooks/alerts/) — what to do when Alertmanager pages you. Indexed in [`runbooks/alerts/README.md`](./runbooks/alerts/README.md).

---

## Bring-up + deploy

Read these in roughly this order when standing up a fresh chain instance.

| Doc | Covers |
|---|---|
| [`quickstart-partner.md`](./quickstart-partner.md) | First-look for a partner or design-partner integrator — what the chain is, how to call it, where to send `register_schema` traffic |
| [`devnet.md`](./devnet.md) | Localnet bring-up (Mock DA) and the federated-devnet design (attestor orgs, multi-sequencer topology — forward-looking) |
| [`public-devnet-deploy.md`](./public-devnet-deploy.md) | GCP VM bring-up procedure for `ligate-devnet-3`: project, SA, firewall, systemd unit, first boot |
| [`celestia-ops.md`](./celestia-ops.md) | Co-located Celestia light node setup, wallet wiring, TIA funding |
| [`snapshot-bootstrap.md`](./snapshot-bootstrap.md) | Spin up a read-only sync node that imports from a public snapshot instead of replaying DA from genesis. For design partners, auditors, third-party node operators. |
| [`canonical-schema-registration.md`](./canonical-schema-registration.md) | Post-genesis schema registration ceremony: bootstrap-cli flow for `themisra.proof-of-prompt/v1` etc. |

## Reference + design

| Doc | Covers |
|---|---|
| [`error-coverage.md`](./error-coverage.md) | REST error envelope: every typed `GenesisError`, `CallError`, `BlobSubmissionError` shape the chain emits |
| [`genesis-determinism.md`](./genesis-determinism.md) | Cross-OS reproducibility for `ligate-genesis-tool generate`; sources of non-determinism we've mitigated |
| [`cost-monitoring.md`](./cost-monitoring.md) | Mocha TIA spend + GCP infra spend visibility (Grafana dashboard, Prometheus alerts, GCP billing export) |
| [`genesis.example.json`](./genesis.example.json) | Example genesis bundle — schema reference for the 8 module JSONs |

## Runbooks

Operational procedures. Each is dated to its last review and pins a "Status: vN" header so changes are obvious.

| Runbook | When to use |
|---|---|
| [`runbooks/mocha-smoke.md`](./runbooks/mocha-smoke.md) | Pre-devnet-announce one-time smoke against real Mocha. Also describes the recurring nightly CI version |
| [`runbooks/celestia-light-node.md`](./runbooks/celestia-light-node.md) | Light-node failure modes, sync recovery, wallet rotation |
| [`runbooks/da-signer-rotation.md`](./runbooks/da-signer-rotation.md) | Rotate the Celestia hot key signing blob submissions; TIA balance monitoring + top-up procedure |
| [`runbooks/sequencer-key-rotation.md`](./runbooks/sequencer-key-rotation.md) | Rotate `sequencer.signer_private_key` on the operator's filesystem |
| [`runbooks/chain-id-bump.md`](./runbooks/chain-id-bump.md) | Move from one chain id to the next (e.g. `devnet-1` → `devnet-2` after a reset) |
| [`runbooks/faucet-ops.md`](./runbooks/faucet-ops.md) | `ligate-api`'s faucet endpoint operation: hot key, rate limits, per-address tracking |
| [`runbooks/backup-restore.md`](./runbooks/backup-restore.md) | GCS-backed `ligate-node` RocksDB snapshot + restore; quarterly drill cadence |
| [`runbooks/log-shipping.md`](./runbooks/log-shipping.md) | Fluent Bit deploy on the chain VM; Cloud Logging filter patterns |
| [`runbooks/multi-sequencer-failover.md`](./runbooks/multi-sequencer-failover.md) | DbElected mode operations: cluster state read, planned Leader replacement, unplanned failover, forced takeover, Postgres outage handling |

## Alerts

Each Prometheus alert has its own runbook describing what fired, why, and the recovery procedure. Index at [`runbooks/alerts/README.md`](./runbooks/alerts/README.md). Quick links:

- [`block-height-stall.md`](./runbooks/alerts/block-height-stall.md) — chain stopped producing blocks
- [`health-down.md`](./runbooks/alerts/health-down.md) — `/metrics` scrape failing
- [`ready-false.md`](./runbooks/alerts/ready-false.md) — node alive but not ready
- [`da-submission-failures.md`](./runbooks/alerts/da-submission-failures.md) — blob submissions to Celestia failing
- [`da-finalization-latency-high.md`](./runbooks/alerts/da-finalization-latency-high.md) — DA path p95 > 60s
- [`mempool-deep.md`](./runbooks/alerts/mempool-deep.md) — sequencer accepting txs faster than packing
- [`disk-filling-fast.md`](./runbooks/alerts/disk-filling-fast.md) — RocksDB growth rate spike

## Cross-references

- [`localnet/README.md`](../../localnet/README.md) — localnet config + dev-key documentation
- [`devnet-1/README.md`](../../devnet-1/README.md) — public-devnet config + genesis substitution flow
- [`ops/`](../../ops/) — Prometheus rules, Alertmanager config, Fluent Bit config, backup scripts, GCS lifecycle policies
- [`ops/grafana/`](../../ops/grafana/) — all four Grafana dashboard JSONs (engineering / operator / investor / cost) plus the README documenting ownership boundaries
- [Issue #316](https://github.com/ligate-io/ligate-chain/issues/316) — ops deploy punchlist umbrella (which of these runbooks have been executed against the live chain)
