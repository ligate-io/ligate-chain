# Alert runbooks

One file per Prometheus alert defined in [`ops/prometheus/alerts.yaml`](../../../../ops/prometheus/alerts.yaml). Each file follows the same shape:

```
1. What fired      — the rule + threshold in plain English
2. Likely causes   — ranked by frequency, not severity
3. First checks    — the cheapest signals to confirm root cause
4. Fix             — what to do for each cause
5. False positives — what looks like a fire but isn't
6. Escalation      — when + how to wake someone up
```

The `runbook_url` annotation on each alert points the on-call at the right file via the firing Slack/email notification.

## Alert catalog

| Alert | Severity | Runbook |
|---|---|---|
| `BlockHeightStall` | page | [`block-height-stall.md`](./block-height-stall.md) |
| `HealthDown` | page | [`health-down.md`](./health-down.md) |
| `ReadyFalse` | warn | [`ready-false.md`](./ready-false.md) |
| `MempoolDeep` | warn | [`mempool-deep.md`](./mempool-deep.md) |
| `DiskFillingFast` | warn | [`disk-filling-fast.md`](./disk-filling-fast.md) |
| `DASubmissionFailures` | page | [`da-submission-failures.md`](./da-submission-failures.md) |
| `DAFinalizationLatencyHigh` | warn | [`da-finalization-latency-high.md`](./da-finalization-latency-high.md) |

## On-call rota (pre-devnet)

Stefan is the single point of contact. Phone + email always reachable. Slack channel `#devnet-ops` is the primary incident surface. Mainnet rota expansion deferred until a second operator exists.

## Deploying these rules + receivers

1. Render `ops/prometheus/alertmanager.yaml` with the secrets (Slack webhook, SMTP password) injected at the file paths the config expects. The repo's copy is the template; the deployed copy is the rendered version.
2. Set `--config.file=alerts.yaml` on the Prometheus server (or include via `rule_files:` in the main config).
3. Set `--config.file=alertmanager.yaml` on the Alertmanager.
4. Confirm Prometheus's `Alerts` tab lists each rule + shows `Inactive`. Then test the wiring by killing `ligate-node` and confirming `HealthDown` fires within 90s + the Slack notification arrives.

The actual deploy of Prometheus + Alertmanager lives outside this repo (operator's monitoring stack, e.g. on the same GCP VM as `ligate-node` or on a separate ops VM). [`public-devnet-deploy.md`](../../public-devnet-deploy.md) covers the broader monitoring story.
