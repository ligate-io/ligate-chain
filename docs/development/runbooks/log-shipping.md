# Runbook — Centralized log shipping (GCP Cloud Logging)

**Status:** v0 — ships the Fluent Bit agent config + the operator-side install for `ligate-devnet-2`. Decision recorded below; alternatives considered + rejected.

The devnet VM (per [`public-devnet-deploy.md`](../public-devnet-deploy.md)) runs `ligate-node`, the Celestia light node, the faucet, plus oneshot backup/snapshot jobs. Without centralized logging, debugging a stall means SSHing into the VM, running `journalctl` across N units, and grepping for the panic. With centralized logging, the operator runs one Cloud Logging query and sees everything.

---

## Decision: GCP Cloud Logging (Option B in #269)

| | Loki (self-hosted) | GCP Cloud Logging |
|---|---|---|
| Infra cost | One more VM to run + back up + alert on | Zero; bundled with GCP |
| Setup time | ~2 hours | ~20 minutes |
| Query UX | Grafana Logs panel (decent) | Logs Explorer (very good) |
| Retention | Operator-managed | Default 30 days; configurable |
| Lock-in | Low (Loki is portable) | High (GCP-specific query syntax + APIs) |
| Migration cost | n/a | Real (would need to reindex or accept history loss) |

**Choice: Cloud Logging.** Lowest-infra path for v0 devnet; the migration risk is real but tractable (logs are append-only, the chain itself + the source code stay portable). Mainnet may revisit if we hit the cost wall or want to leave GCP.

---

## What gets shipped

Fluent Bit reads from `systemd-journald` and ships to Cloud Logging. Sources:

| Tag | systemd unit | What |
|---|---|---|
| `ligate-node` | `ligate-node.service` | The chain process (`tracing_subscriber` JSON output) |
| `celestia-light` | `celestia-light.service` | DA light node (Go-style slog) |
| `ligate-faucet` | `ligate-faucet.service` | Faucet (if co-located; today lives in ligate-api on Railway) |
| `ligate-backup` | `ligate-backup.service` | Oneshot backup runs |
| `ligate-snapshot` | `ligate-snapshot-publish.service` | Oneshot public-snapshot runs |

Fluent Bit parses the JSON from `ligate-node` (tracing's `format::json()`) and forwards everything else as plain text. Cloud Logging keys off the `level` field for severity mapping (`ERROR` → Cloud Logging's `ERROR`, etc.) and the `target` field for source-module filtering.

---

## One-time setup

### 1. Cloud Logging scopes on the VM

The VM's service account needs the `logging.logWriter` role. If the VM was provisioned with the default Compute Engine service account, it already has this; otherwise:

```sh
gcloud projects add-iam-policy-binding "$GCP_PROJECT" \
    --member="serviceAccount:<vm-service-account>" \
    --role="roles/logging.logWriter"
```

### 2. Install Fluent Bit on the VM

```sh
# Debian/Ubuntu via the official repo
curl https://packages.fluentbit.io/fluentbit.key | sudo gpg --dearmor -o /usr/share/keyrings/fluentbit-keyring.gpg
echo "deb [signed-by=/usr/share/keyrings/fluentbit-keyring.gpg] https://packages.fluentbit.io/debian/bookworm bookworm main" \
    | sudo tee /etc/apt/sources.list.d/fluentbit.list
sudo apt-get update
sudo apt-get install -y fluent-bit

# Place our config
sudo install -m 0644 ops/logging/fluent-bit.conf /etc/fluent-bit/fluent-bit.conf
sudo install -m 0644 ops/logging/parsers.conf /etc/fluent-bit/parsers.conf

# Restart
sudo systemctl enable --now fluent-bit
journalctl -fu fluent-bit
```

Confirm with a one-shot test in Cloud Logging Logs Explorer:

```
resource.type="gce_instance"
labels.host="<vm-hostname>"
log_name=~"ligate-.*"
```

Should return recent journal lines from the ligate units within ~10s.

### 3. Add to cloud-init for new VMs

Append to the `runcmd:` block in the cloud-init template (per `public-devnet-deploy.md` Step 2):

```yaml
  # Fluent Bit log shipping → Cloud Logging
  - |
    curl https://packages.fluentbit.io/fluentbit.key | gpg --dearmor -o /usr/share/keyrings/fluentbit-keyring.gpg
    echo "deb [signed-by=/usr/share/keyrings/fluentbit-keyring.gpg] https://packages.fluentbit.io/debian/bookworm bookworm main" \
      > /etc/apt/sources.list.d/fluentbit.list
    apt-get update
    apt-get install -y fluent-bit
  # Operator places ops/logging/fluent-bit.conf + parsers.conf at
  # /etc/fluent-bit/ post-cloud-init (configs ship in the chain repo)
  - systemctl enable --now fluent-bit
```

---

## ligate-node logging config

`ligate-node` writes JSON-structured logs via `tracing-subscriber` so Fluent Bit can pull structured fields without regex parsing. Confirm with:

```sh
journalctl -u ligate-node -n 5 -o cat | jq -r '.'
```

Each line should parse cleanly. If lines come through unparsed (e.g. `pretty()` format), the chain's log-init in `crates/rollup/src/main.rs` is misconfigured for production; the JSON formatter is the right call for non-local builds.

The relevant fields:

| Field | Source | Use |
|---|---|---|
| `timestamp` | RFC3339-ms UTC | Time-axis filtering |
| `level` | `TRACE`/`DEBUG`/`INFO`/`WARN`/`ERROR` | Severity filter |
| `target` | Rust module path | Filter by component (e.g. `target=~"sov_blob_sender.*"`) |
| `fields` | per-event structured data | Query by attribute (e.g. `tx_hash` on submission events) |
| `spans` | tracing span stack | Trace request flow across modules |

---

## Query examples

In the Cloud Logging Logs Explorer:

**All errors from ligate-node in the last hour:**
```
log_name="projects/${PROJECT}/logs/ligate-node"
severity>="ERROR"
timestamp>="-1h"
```

**Recent DA submission events:**
```
log_name=~"ligate-node|celestia-light"
jsonPayload.target=~".*blob_sender.*|.*celestia.*"
```

**All log lines for a specific tx hash (across services):**
```
jsonPayload.fields.tx_hash="ltx1abc..."
```

**Sequencer panics (last 24h):**
```
log_name="projects/${PROJECT}/logs/ligate-node"
jsonPayload.level="ERROR"
jsonPayload.fields.panic=true
timestamp>="-24h"
```

---

## Retention + cost

Default Cloud Logging retention is 30 days for the `_Default` log bucket. ligate-node's log volume at devnet-1 expected ~10-50 MB/day; well within the free tier (50 GB/month) so cost is $0 pre-devnet and tracks $0.50/GB above.

Tune retention via:

```sh
# Reduce retention if log volume grows
gcloud logging buckets update _Default --location=global --retention-days=14
```

Custom retention buckets per log type are post-devnet hygiene; for now `_Default` is fine.

---

## What this does NOT cover

- **Distributed tracing.** `tracing-subscriber` spans show in the logs but full trace correlation across services (OpenTelemetry / Jaeger) is a separate workstream. Deferred past devnet.
- **Log-derived metrics.** Already covered by Prometheus (#110, #164). Avoid duplicating in Cloud Logging.
- **Alerting on logs.** Possible via Cloud Logging's log-based metrics + Alertmanager hook, but the current alerts (#268) all source from Prometheus. Revisit if a useful alert can ONLY be derived from log content.
- **Multi-VM aggregation.** Single-sequencer devnet means one VM today. Adding followers means installing Fluent Bit on each + filtering by `labels.host` in the query. No additional config needed.

---

## Cross-references

- [Issue #269](https://github.com/ligate-io/ligate-chain/issues/269) — this runbook closes it
- [`public-devnet-deploy.md`](../public-devnet-deploy.md) — the VM bring-up that consumes the fluent-bit install
- [`alerts/`](./alerts/) — Prometheus alerting (paired layer of observability)
- [`ops/logging/fluent-bit.conf`](../../../ops/logging/fluent-bit.conf) + [`parsers.conf`](../../../ops/logging/parsers.conf) — the agent configs
