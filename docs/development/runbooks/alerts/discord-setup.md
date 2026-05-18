# Discord alert wiring

How to wire `ops/prometheus/alerts.yaml` firings into the Ligate Labs Discord (`#chain-alerts`) via Grafana Cloud's native Discord contact point. This is the canonical path for the public devnet operator; the self-hosted Alertmanager flow at `ops/prometheus/alertmanager.yaml` stays in the repo as a fallback (see [§Self-hosted Alertmanager fallback](#self-hosted-alertmanager-fallback)).

Tracking issue: [#268](https://github.com/ligate-io/ligate-chain/issues/268).

## Why Discord

Pre-mainnet, we are not paying for PagerDuty. The Ligate Labs Discord (permanent invite `https://discord.gg/ZWUeJ8k3eP`) is already where the team hangs out; the same surface gets partner DMs, devnet status posts, and now ops alerts. A Discord webhook is free, has no per-seat cost, and Grafana Cloud Alerting ships a native Discord contact point so we do not need to run a bridge process. Mainnet escalation (PagerDuty rotas, severity-specific paging policies) is future work; this wiring keeps the same `severity=page|warn|info` labels so the routing rules carry forward.

The chain VM already pushes metrics to Grafana Cloud via `/etc/alloy/config.alloy`'s `prometheus.remote_write "grafana_cloud"`; alert evaluation happens Grafana-side, which is why we do not currently run our own Alertmanager (the in-repo `ops/prometheus/alertmanager.yaml` is documentation + fallback only).

## Step 1: Create the webhook in Discord

In the Ligate Labs Discord server:

1. Server Settings (top of the server list, the down-arrow next to the server name).
2. Integrations (left sidebar).
3. Webhooks (panel on the right) → New Webhook.
4. Name: `chain-alerts`.
5. Channel: `#chain-alerts`. If the channel does not exist yet, create it first (Server Settings → cancel back to the server, right-click in the channel list → Create Channel → Text → `chain-alerts` → Create; private to the `@chain-ops` role if you want operator-only visibility, public otherwise).
6. Copy Webhook URL. You will not see it again without clicking "Copy" on the webhook entry; rotate it via "Reset" if it leaks.
7. Store at `~/.config/ligate/secrets.env`:

   ```sh
   DISCORD_ALERTS_WEBHOOK_URL=https://discord.com/api/webhooks/<id>/<token>
   ```

   `chmod 600 ~/.config/ligate/secrets.env`. Never paste the URL into chat, a Slack message, or a PR description; the URL itself is the credential.

## Step 2: Grafana Cloud contact point

Grafana Cloud Alerting ships a native Discord contact point; no bridge process needed.

1. Grafana Cloud → Alerting (bell icon in the left sidebar) → Contact points.
2. + Add contact point.
3. Name: `discord-chain-alerts`.
4. Integration: Discord.
5. Webhook URL: paste from `DISCORD_ALERTS_WEBHOOK_URL` (Step 1).
6. Message template (optional but recommended; pastes inline rather than using a separate template file):

   ```
   **{{ .CommonLabels.alertname }}** ({{ .CommonLabels.severity }})

   {{ range .Alerts }}
   - Instance: `{{ .Labels.instance }}`
   - Summary: {{ .Annotations.summary }}
   - Runbook: {{ .Annotations.runbook_url }}
   {{ end }}
   ```

7. Avatar URL (optional): point at the Ligate mark on the marketing site, e.g. `https://ligate.io/og-default.png`.
8. Save contact point → Test (sends a synthetic firing alert to the channel; if `#chain-alerts` does not receive it within ~10s, re-check the webhook URL).

## Step 3: Notification policy

Three severity tiers map to three notification behaviours:

1. Grafana Cloud → Alerting → Notification policies.
2. + New nested policy.
3. **`severity=page` policy** (operator must look):
   - Matching labels: `severity = page`
   - Contact point: `discord-chain-alerts`
   - Override grouping: by `alertname, instance` (matches `alertmanager.yaml`'s `group_by`).
   - Override timings: group wait 30s, group interval 5m, repeat interval 4h.
   - Mute timings: none.
   - **Mention**: under "Override message template" or the contact-point-level template, add `@here` to the prefix. Grafana's Discord integration sends the template body as the message content, so `@here` in the template body resolves correctly in the channel.
4. **`severity=warn` policy** (business-hours):
   - Matching labels: `severity = warn`
   - Same contact point, same grouping.
   - No `@here` mention; just the alert body.
   - Repeat interval 4h.
5. **`severity=info` policy** (channel-only):
   - Matching labels: `severity = info`
   - Same contact point.
   - No mention.
   - "Send resolved" toggled OFF (skip the auto-resolve "OK" message; info-tier does not need closure noise).

## Step 4: Import alert rules from `ops/prometheus/alerts.yaml`

Grafana Cloud's alert rules editor accepts Prometheus-format expressions directly; the rules in `ops/prometheus/alerts.yaml` paste in as-is. The `runbook_url` annotation flows through to the Discord message via the template above.

Walkthrough for `BlockHeightStall` (the cheapest one to verify against; the others follow the same shape):

1. Alerting → Alert rules → + New alert rule.
2. Rule name: `BlockHeightStall`.
3. Rule type: Grafana-managed alert (the rules-section UI; not the data-source-managed one).
4. Define query → Data source: `grafanacloud-ligate-prom` (the Prometheus instance the chain VM remote-writes to).
5. Expression (paste from `ops/prometheus/alerts.yaml`):

   ```promql
   changes(ligate_block_height[2m]) == 0
   and on(instance) (ligate_block_height > 0)
   ```

6. Condition: `WHEN last() OF A IS ABOVE 0` (Grafana wraps the PromQL output in an `A` reference; threshold > 0 means "the expression returned at least one series", which is how Grafana models the Prometheus "the rule is firing" boolean).
7. Folder: `Ligate Chain alerts` (create if missing). Evaluation group: `ligate-node.liveness` (mirror the group names from `alerts.yaml`). Evaluation interval: 30s. Pending period: 60s (matches the rule's `for: 60s`).
8. Annotations:
   - `summary`: `Block height unchanged for 60s on {{ $labels.instance }}`
   - `description`: paste the multi-line description from `alerts.yaml`.
   - `runbook_url`: `https://github.com/ligate-io/ligate-chain/blob/main/docs/development/runbooks/alerts/block-height-stall.md`
9. Labels:
   - `severity = page`
   - `component = chain`
10. Save rule.

Repeat for the remaining nine rules. The full set, with severities and per-rule runbook paths, is in [§What gets paged vs channel-only](#what-gets-paged-vs-channel-only) below.

## Step 5: Test fire

Synthetic fire to confirm the webhook → channel path (does NOT exercise the Grafana evaluation pipeline; this is the smoke test on the last hop):

```sh
source ~/.config/ligate/secrets.env
curl -X POST "$DISCORD_ALERTS_WEBHOOK_URL" \
  -H 'Content-Type: application/json' \
  -d '{"content":"test fire from chain-alerts setup"}'
```

Expected: the message shows up in `#chain-alerts` within a couple of seconds. If not, re-check the URL is the full `https://discord.com/api/webhooks/<id>/<token>` form (not truncated), and that the webhook still exists in the Discord integration list (rotate / recreate if it was deleted).

To exercise the full evaluation path (Prometheus rule → Grafana contact point → Discord), kill `ligate-node` on the dev VM and wait for `HealthDown` to trip:

```sh
sudo systemctl stop ligate-node
# wait ~30-60s for the `for: 30s` to expire and the scrape to fail
# `HealthDown` posts to `#chain-alerts` with the `@here` mention
sudo systemctl start ligate-node
# the OK auto-resolve fires once `up` returns to 1, ~30s after restart
```

## What gets paged vs channel-only

Severity comes from `ops/prometheus/alerts.yaml`'s `labels.severity`. Page = `@here` mention; warn = channel only; info = channel only, no resolve message.

| Alert | Severity | Behaviour in `#chain-alerts` | Runbook |
|---|---|---|---|
| `BlockHeightStall` | page | `@here` mention, repeats every 4h | [`block-height-stall.md`](./block-height-stall.md) |
| `HealthDown` | page | `@here` mention, repeats every 4h | [`health-down.md`](./health-down.md) |
| `DASubmissionFailures` | page | `@here` mention, repeats every 4h | [`da-submission-failures.md`](./da-submission-failures.md) |
| `TIABalanceCritical` | page | `@here` mention, repeats every 4h | (covered in `da-signer-rotation.md`) |
| `ReadyFalse` | warn | channel post, no mention | [`ready-false.md`](./ready-false.md) |
| `MempoolDeep` | warn | channel post, no mention | [`mempool-deep.md`](./mempool-deep.md) |
| `DiskFillingFast` | warn | channel post, no mention | [`disk-filling-fast.md`](./disk-filling-fast.md) |
| `DAFinalizationLatencyHigh` | warn | channel post, no mention | [`da-finalization-latency-high.md`](./da-finalization-latency-high.md) |
| `TIABalanceLow` | warn | channel post, no mention | (covered in `da-signer-rotation.md`) |
| `TIADrawdownSpike` | warn | channel post, no mention | (covered in `cost-monitoring.md`) |

No alerts ship at `info` today; the tier is reserved for future "interesting but not actionable" signals (e.g. attestor-set rotation completed, scheduled re-genesis cutover hit).

## Self-hosted Alertmanager fallback

For operators running their own monitoring stack (follower nodes, third-party partners spinning up their own attestor sets, anyone who does not want a Grafana Cloud account), the canonical path is the `ops/prometheus/alertmanager.yaml` config in this repo. That file ships with three receivers: the default Slack route, a page-severity email route, and (added by this change) a Discord webhook route.

Alertmanager itself does not have a native Discord integration; the standard pattern is to point Alertmanager's `webhook_configs` at the [`alertmanager-discord`](https://github.com/benjojo/alertmanager-discord) Go binary running as a sidecar on the same host. The bridge reads `DISCORD_WEBHOOK` from its environment (set to the same URL Step 1 stored) and translates Alertmanager's JSON payload into a Discord-compatible message. The sidecar is ~5MB, has no persistent state, and runs as a systemd unit alongside Alertmanager.

The receiver block added to `ops/prometheus/alertmanager.yaml` documents the `webhook_configs` shape and points at `http://127.0.0.1:9094/` (the bridge's default listen address). Operators wiring the fallback path should add an `alertmanager-discord.service` systemd unit, hydrate `DISCORD_WEBHOOK` from their secret store, and start it before Alertmanager. The canonical Grafana Cloud path above does not need any of this.
