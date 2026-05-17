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

The `runbook_url` annotation on each alert points the on-call at the right file via the firing Discord / Slack / email notification.

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
| `TIABalanceLow` | warn | (covered in [`da-signer-rotation.md`](../da-signer-rotation.md#tia-balance-monitoring)) |
| `TIABalanceCritical` | page | (covered in [`da-signer-rotation.md`](../da-signer-rotation.md#tia-balance-monitoring)) |
| `TIADrawdownSpike` | warn | (covered in [`cost-monitoring.md`](../../cost-monitoring.md)) |

## Notification wiring

How firings reach an operator is documented in [`discord-setup.md`](./discord-setup.md): the canonical path is Grafana Cloud Alerting → native Discord contact point → `#chain-alerts` in the Ligate Labs Discord. Page-severity alerts get an `@here` mention; warn-severity is channel-only. For self-hosted Alertmanager deploys (followers, third-party partners), [`discord-setup.md`](./discord-setup.md#self-hosted-alertmanager-fallback) covers the `alertmanager-discord` bridge pattern that pairs with `ops/prometheus/alertmanager.yaml`.

## On-call rota (pre-devnet)

Stefan is the single point of contact. Phone + email always reachable. Discord channel `#chain-alerts` is the primary incident surface; `#devnet-ops` Slack stays in the routing tree as a secondary surface. Mainnet rota expansion deferred until a second operator exists.

## Deploying these rules + receivers

Canonical path (Grafana Cloud): follow [`discord-setup.md`](./discord-setup.md) Steps 1-4 to wire the webhook, contact point, notification policy, and import the rules from `ops/prometheus/alerts.yaml`. Step 5 covers end-to-end test fire.

Self-hosted fallback (own Prometheus + Alertmanager stack):

1. Render `ops/prometheus/alertmanager.yaml` with the secrets (Slack webhook, SMTP password, Discord bridge URL) injected at the file paths the config expects. The repo's copy is the template; the deployed copy is the rendered version.
2. Set `--config.file=alerts.yaml` on the Prometheus server (or include via `rule_files:` in the main config).
3. Set `--config.file=alertmanager.yaml` on the Alertmanager.
4. If routing to Discord, stand up the `alertmanager-discord` bridge sidecar with `DISCORD_WEBHOOK=$DISCORD_ALERTS_WEBHOOK_URL` per [`discord-setup.md` §Self-hosted Alertmanager fallback](./discord-setup.md#self-hosted-alertmanager-fallback).
5. Confirm Prometheus's `Alerts` tab lists each rule + shows `Inactive`. Then test the wiring by killing `ligate-node` and confirming `HealthDown` fires within 90s + the Discord notification arrives.

The actual deploy of Prometheus + Alertmanager lives outside this repo (operator's monitoring stack, e.g. on the same GCP VM as `ligate-node` or on a separate ops VM). [`public-devnet-deploy.md`](../../public-devnet-deploy.md) covers the broader monitoring story.
