# celestia-tia-exporter

Tiny Prometheus exporter that exposes the chain's DA-signer TIA balance as `celestia_node_da_signer_balance_utia`. Plugs the gap that `celestia-node` v0.30.x only ships OTLP metrics (no native Prometheus exposition), which means the `TIABalanceLow` / `TIABalanceCritical` / `TIADrawdownSpike` alerts in `ops/prometheus/alerts.yaml` would otherwise never fire.

## What it does

1. On startup, mints an admin JWT via `celestia light auth admin` (requires read access to the light-node keystore).
2. Polls `state.BalanceForAddress` against the chain-recorded `seq_da_address` (NOT `state.Balance` against the light-node default account, which often holds zero).
3. Exposes the balance in utia (micro-TIA) on `http://127.0.0.1:9101/metrics` for Alloy/Prometheus to scrape.
4. On 401 (token revoked / wallet keyring rotated), re-mints the JWT and retries once.

## Install on the chain VM

```sh
# 1. Drop the script in place.
sudo mkdir -p /opt/ligate/exporters/celestia-tia
sudo install -m 0755 ops/exporters/celestia-tia/celestia-tia-exporter.py \
    /opt/ligate/exporters/celestia-tia/

# 2. Drop the systemd unit + env file.
sudo install -m 0644 ops/exporters/celestia-tia/systemd/celestia-tia-exporter.service \
    /etc/systemd/system/
sudo install -m 0640 -o ligate -g ligate ops/exporters/celestia-tia/systemd/celestia-tia-exporter.env.example \
    /etc/ligate/celestia-tia-exporter.env
# Edit DA_SIGNER_ADDRESS if devnet-1/genesis/sequencer_registry.json `seq_da_address` changes.

# 3. Enable + start.
sudo systemctl daemon-reload
sudo systemctl enable --now celestia-tia-exporter.service

# 4. Verify locally.
curl -s http://127.0.0.1:9101/metrics | grep celestia_node_da_signer_balance_utia
# expect: a single gauge line, value in utia (e.g. 12956768 == 12.956768 TIA)

# 5. Add the scrape block to /etc/alloy/config.alloy:
#
#   prometheus.scrape "celestia_tia" {
#     targets = [{
#       __address__ = "127.0.0.1:9101",
#       job         = "celestia-tia",
#       instance    = "ligate-devnet-2-sequencer",
#     }]
#     scrape_interval = "60s"
#     forward_to = [prometheus.remote_write.grafana_cloud.receiver]
#   }
#
# Then reload Alloy:
sudo systemctl reload alloy
```

## Verify in Grafana Cloud

After ~1-2 min, the metric should be queryable:

```promql
celestia_node_da_signer_balance_utia / 1e6
```

That returns the balance in TIA. The matching alert rules already exist in `ops/prometheus/alerts.yaml`; they'll start evaluating on the next scrape.

## Why a sidecar exporter and not native scraping

Celestia-node 0.30.x's `--metrics` flag enables OTLP HTTP export to an OpenTelemetry collector (default `localhost:4318`). To get those into Prometheus you'd need to run an OTLP-to-Prometheus collector (typically `otelcol-contrib`) — that's heavier infrastructure than the operational value justifies for a single gauge.

This exporter is ~250 lines of stdlib Python, runs as `ligate` user with `NoNewPrivileges` + `ProtectSystem=strict`, and only needs read access to the light-node keystore. Worth the trade.

## Operational notes

- **`DA_SIGNER_ADDRESS` matters.** If you point this at the wrong address (e.g. the light node's default `state.AccountAddress`), the metric will always read 0 and look broken. The chain's actual submitter address is recorded in `devnet-1/genesis/sequencer_registry.json` field `seq_da_address`. If that's rotated (per `docs/development/runbooks/da-signer-rotation.md`), update this env var too.
- **Token never expires by default.** Celestia issues admin JWTs with `ExpiresAt: 0001-01-01T00:00:00Z` (zero value). The 401-handler is belt-and-braces for the case where the keystore is rotated or the keys-dir wiped.
- **Polling cadence is 60s.** Tighter is allowed (the alerts use a 30m / 5m evaluation window so the staleness is dominated by Prometheus eval, not exporter poll). Looser is also fine.

## Companion alerts

The exporter exists so these (already in `ops/prometheus/alerts.yaml`) actually fire:

- `TIABalanceLow` (warn): balance < 50 TIA sustained 30m
- `TIABalanceCritical` (page): balance < 12 TIA sustained 5m
- `TIADrawdownSpike` (warn): 24h burn > 5 TIA/day

Runbook: `docs/development/runbooks/alerts/README.md` and the per-alert sub-runbooks.
