# Runbook, Incident Response

**Status:** v0, pre-mainnet single-operator playbook. Roles consolidate to Stefan today; named-role split lands once a second operator joins.
**Tracks:** [chain#513](https://github.com/ligate-io/ligate-chain/issues/513), child of [chain#360](https://github.com/ligate-io/ligate-chain/issues/360)

This is the "what we do in the moment" playbook. The "what we do after" playbook is [`disaster-recovery.md`](./disaster-recovery.md). The "what we do before any of this" preparation is in [`backup-restore.md`](./backup-restore.md) (quarterly restore drill), [`gcp-audit-logs.md`](./gcp-audit-logs.md) (forensic trail enabled), and [`operator-key-rotation.md`](./operator-key-rotation.md) (rotation procedure rehearsed).

---

## Roles (single-operator stage)

Until a second operator joins, all four roles below collapse to Stefan. The role names exist so the procedure scales when more operators come online.

| Role | Responsibility | Today |
|------|----------------|-------|
| **Incident Commander (IC)** | Drives the incident: declares severity, picks the procedure, calls the all-clear | Stefan |
| **Comms** | External status page + X + partner DMs | Stefan |
| **Forensics** | Pulls audit logs, RocksDB state, chain-side evidence | Stefan |
| **Ops Lead** | Executes the mechanical fix steps (restart, restore, rotate) | Stefan |

The IC is the single point of decision-making during an incident. If the IC needs to step into a fix-step, hand off the IC role explicitly before doing so. The IC role is never silently empty.

---

## Severity ladder

| Sev | Definition | Response time target | Comms |
|-----|------------|----------------------|-------|
| **SEV-1** | Chain stopped, key compromised, treasury at risk, or any state-integrity issue | <15 min ack, declare publicly within 1h | Status page + X post + partner DMs |
| **SEV-2** | Service degraded but chain healthy (RPC errors, faucet drained, indexer lag) | <1h ack, fix in same business day | Status page update, no public X |
| **SEV-3** | Cosmetic or single-partner impact, no broader user impact | Next business day | Internal note, no public update |

Default toward upgrading severity if uncertain. Downgrading mid-incident is fine; not noticing a SEV-1 because it looked SEV-2 is not.

---

## Comms templates

### Status page (SEV-1, initial)

```
[Investigating] Ligate Chain devnet-2 is experiencing <one-sentence symptom>.
We are investigating. Updates every 30 minutes.
Last update: <timestamp UTC>
```

### Status page (SEV-1, root cause known)

```
[Identified] Ligate Chain devnet-2 incident root cause: <one sentence>.
Mitigation in progress: <one sentence on what we are doing>.
ETA to resolution: <best-effort estimate, do not over-promise>.
Last update: <timestamp UTC>
```

### Status page (resolved)

```
[Resolved] The Ligate Chain devnet-2 incident from <start UTC> to <end UTC>
was caused by <one-sentence root cause>. Mitigation: <one sentence>.
Post-mortem to follow within 72 hours.
```

### X post (SEV-1, single tweet)

> Ligate Chain devnet-2 had an incident from <start> to <end> UTC. Root cause: <X>. Mitigation: <Y>. Detailed write-up coming this week.

Don't tweet during the incident unless we have something to say. Silence is better than speculation.

### Partner DM (SEV-1)

> Hey, heads up, devnet-2 had a <X-type> incident at <UTC>. We've mitigated; chain is healthy now. <If they have on-chain dependencies>: your <integration name> may have seen <specific impact, e.g. failed transactions between time A and time B>. Happy to dig in if you saw issues; otherwise we'll share the full post-mortem in <N> days.

Be specific about impact. Vague reassurance reads as cover.

---

## Scenario 1, sequencer hard-down

Symptoms: `/v1/ledger/slots/latest` not advancing, `systemctl status ligate-node` shows failed or dead, no batches landing on Celestia.

### Diagnosis (5 min)

```sh
# On the sequencer VM
sudo systemctl status ligate-node
sudo journalctl -u ligate-node -n 200 --no-pager
df -h /var/lib/ligate  # disk full?
free -h                # OOM?
```

External view:
```sh
curl -s https://rpc.ligate.io/v1/ledger/slots/latest | jq .height
# Compare to height 5 minutes ago in Grafana
```

### Mitigation by failure mode

| Cause | Fix | Reference |
|-------|-----|-----------|
| Process crashed cleanly | `sudo systemctl restart ligate-node`, watch logs | [`chain-upgrade.md`](./chain-upgrade.md) |
| Crash loop on start | Roll back to previous binary, file bug with stacktrace | [`chain-upgrade.md`](./chain-upgrade.md) auto-rollback path |
| Disk full | Free space (`/var/log` rotation, old snapshots in `/tmp`), restart | One-off |
| RocksDB corruption | Restore from snapshot | [`backup-restore.md`](./backup-restore.md) |
| VM dead | Failover to standby (when multi-sequencer ships) or rebuild | [`multi-sequencer-failover.md`](./multi-sequencer-failover.md) + [`disaster-recovery.md`](./disaster-recovery.md) |

### Declare resolved when

- `/v1/ledger/slots/latest` advances at expected cadence for 5 consecutive minutes
- No `error`-level entries in `journalctl -u ligate-node -n 100` since restart
- Light-node sync gauge healthy in Grafana

---

## Scenario 2, RPC degraded (chain healthy)

Symptoms: `rpc.ligate.io` returns 5xx or high latency, but the sequencer is producing blocks normally.

### Diagnosis (5 min)

```sh
# Is the chain healthy?
curl -s https://rpc.ligate.io/v1/ready
curl -s https://rpc.ligate.io/v1/ledger/slots/latest | jq .height

# Latency from outside
time curl -s https://rpc.ligate.io/v1/rollup/info > /dev/null

# Cloudflare edge state
curl -sI https://rpc.ligate.io/v1/rollup/info | grep -i cf-cache-status
```

Look in Grafana:
- HTTP error rate panel on `rpc-edge` dashboard
- Per-IP rate-limit hit count (Cloudflare WAF panel)

### Mitigation by failure mode

| Cause | Fix |
|-------|-----|
| Burst of legitimate traffic | Cloudflare auto-handles via rate limit; monitor and consider raising the per-IP limit |
| DDoS-shaped pattern | Cloudflare Under Attack Mode (one click in the CF dashboard) |
| Origin overwhelmed (CPU at 100%) | Restart `ligate-node`, scale VM up via [`upgrade-chain.sh`](./chain-upgrade.md) on a larger instance |
| Caddy / reverse proxy down | `sudo systemctl restart caddy` on the sequencer VM |
| Cloudflare DNS or routing issue | Status page update; no fix from our side; wait for CF resolution |

### Declare resolved when

- p99 RPC latency back under 500ms (per Grafana panel)
- HTTP error rate under 1% for 10 consecutive minutes

---

## Scenario 3, faucet drained

Symptoms: `faucet-bot` Discord channel reports empty, `/v1/drip` returns "insufficient balance," operator wallet balance near zero.

### Diagnosis (10 min)

```sh
# Operator balance
ligate-cli balance lid1...operator

# Recent drip txs (look for anomalous pattern)
curl -s "https://api.ligate.io/v1/drip/recent?limit=100" | jq

# Cloudflare per-IP rate limit hit pattern in Grafana
```

Look for:
- A single IP responsible for many drips (rate-limit bypass)
- A small set of recipient addresses receiving repeated drips (drain attack)
- An unexplained large outbound tx from the operator wallet (compromise)

### Mitigation by failure mode

| Cause | Fix |
|-------|-----|
| Routine exhaustion (legitimate demand) | Refill the operator wallet from the cold treasury, document the burn rate in the cost dashboard |
| Drain attack via rate-limit bypass | Tighten Cloudflare rate limit; consider adding captcha or proof-of-work on `/v1/drip`; raise the issue on faucet-bot repo |
| Compromise (large outbound to unknown address) | **Stop, this is SEV-1.** See Scenario 4 below; do NOT refill the wallet until rotation completes |

The key distinction: a slow drain from many addresses is a rate-limit problem. A single large outbound tx is a key compromise. Treat differently.

### Declare resolved when

- Operator wallet balance back above the configured floor
- Cloudflare logs show no anomalous IP pattern in the last hour
- `/v1/drip` returns success for new requests

---

## Scenario 4, suspected operator or DA key compromise

Symptoms: any of (a) large unexplained outbound tx from the operator wallet, (b) signing operation in Cloud Audit Logs that we did not initiate, (c) third-party reports finding our private key material in a leak, (d) an investigator confirms a backup tarball was exfiltrated.

This is SEV-1. Wake up. Do not finish your coffee.

### Step 0, before doing anything irreversible

- **Snapshot the current state** (RocksDB + genesis + config + Secret Manager state). [`backup-restore.md`](./backup-restore.md) procedure. The post-mortem needs the as-of state.
- **Pull the audit log evidence** from the locked retention sink ([`gcp-audit-logs.md`](./gcp-audit-logs.md)). Queries by service, by actor, by time window.

### Step 1, contain (the next 15 minutes)

- **Operator key compromised**: drain the treasury to the cold address per [`operator-key-rotation.md`](./operator-key-rotation.md) Procedure B step 1. Even seconds matter; the adversary is racing you.
- **DA key compromised**: drain the Celestia TIA balance per [`da-signer-rotation.md`](./da-signer-rotation.md) Procedure B.
- Revoke IAM bindings on any compromised service account (`gcloud projects remove-iam-policy-binding`).
- If the compromise traces to a specific VM or laptop, isolate it: `gcloud compute instances stop ligate-sequencer` (for the VM), or revoke SSH keys (for the laptop).

### Step 2, rotate

- Operator: [`operator-key-rotation.md`](./operator-key-rotation.md) Procedure B (emergency under compromise; bumps chain id).
- DA: [`da-signer-rotation.md`](./da-signer-rotation.md) Procedure B.
- Sequencer signing key: [`sequencer-key-rotation.md`](./sequencer-key-rotation.md) Procedure A (re-genesis) for pre-mainnet, Procedure B once that primitive ships.

The rotation procedures are coupled to chain-id-bump. Update downstream services in parallel per the rotation runbook.

### Step 3, comms (within 60 minutes of containment)

- Status page update (template above)
- X post (template above; can wait for full root-cause clarity)
- Partner DMs (the LOI buyer, any active integrators)
- Internal post-mortem doc started immediately, finalised within 72h

### Step 4, post-mortem

Lives at `docs/development/postmortems/<date>-<one-word>.md`. Format:
- Timeline (UTC, every action)
- Root cause analysis (the 5 whys, not "user error")
- What worked in this runbook
- What didn't work (file as a follow-up to update this runbook)
- Action items with owners

---

## Scenario 5, suspected on-chain exploit

Symptoms: an attestation, a schema registration, a transfer, or a bounty interaction that the chain accepts but should have rejected. Discovered via partner report, an alert from the indexer, or a difference between expected and observed module state.

This is SEV-1 and structurally different from Scenarios 1-4: there is nothing wrong with the operator's machine; the chain itself accepted something it shouldn't have.

### Step 0, evidence

- Snapshot the chain state. Same as Scenario 4 step 0.
- Pull the specific tx + the slot it landed in + the attestor / sequencer that included it.

### Step 1, decide whether to halt

This is the IC's hardest call.

| Decide to halt if | Decide not to halt if |
|-------------------|------------------------|
| Continuing harms partners (e.g. token loss in flight, schema being misused at scale) | Exploit is theoretical or limited (a single weird tx with no downstream effect) |
| Fix requires a chain-side patch + restart | Fix can land via a downstream config or module governance call |
| Trust in the chain depends on stopping NOW | Pausing causes more harm than the exploit |

**No protocol-level halt switch ships today.** Halting means stopping `ligate-node` on the sequencer VM. Chain stalls until restart. Be sure before doing this; the comms cost is real.

### Step 2, patch

- Identify the chain-side bug (state machine, module logic, signature verification, etc.)
- Write the fix on a branch, ship a patch release, deploy via [`chain-upgrade.md`](./chain-upgrade.md)
- If the fix changes state machine or genesis, treat as a chain-hash bump (rebuild from DA, see [`chain-id-bump.md`](./chain-id-bump.md))

### Step 3, decide whether to roll back

For mainnet (future): chain-side rollback is a credibility-destroying operation; reserve for actual fund-loss-scale events. For devnet: state is replayable; rollback is "stop, edit genesis or state machine, replay."

### Step 4, comms (within 24h)

- Status page + X post + partner DMs
- Public post-mortem within 7 days
- Bug bounty payout if applicable (program lives at #313 once that ships)

---

## Post-incident checklist (every incident)

After the all-clear:

- [ ] Update the status page to "resolved"
- [ ] X post (within 24h for SEV-1, within 7d for SEV-2)
- [ ] Partner DMs sent
- [ ] Post-mortem started in `docs/development/postmortems/`
- [ ] Action items filed as GitHub issues with the `incident-followup` label (create the label if not present)
- [ ] This runbook updated with any procedure gap discovered (PR + merge before the next incident)
- [ ] Audit log retention sink confirmed populated with the incident's events

The runbook gets better only if we update it after every incident. Don't skip the update step because the incident felt small.

---

## Cross-references

- [`disaster-recovery.md`](./disaster-recovery.md): step-by-step recovery for the failure modes touched on above
- [`backup-restore.md`](./backup-restore.md): RocksDB snapshot + restore procedure
- [`chain-upgrade.md`](./chain-upgrade.md): in-place binary swap with auto-rollback
- [`chain-id-bump.md`](./chain-id-bump.md): genesis + chain id ladder bump
- [`operator-key-rotation.md`](./operator-key-rotation.md): operator wallet key rotation
- [`sequencer-key-rotation.md`](./sequencer-key-rotation.md): chain-side ed25519 signing key rotation
- [`da-signer-rotation.md`](./da-signer-rotation.md): Celestia DA wallet rotation
- [`gcp-audit-logs.md`](./gcp-audit-logs.md): forensic trail queries during the incident
- [`multi-sequencer-failover.md`](./multi-sequencer-failover.md): cluster failover (multi-VM future state)
- [chain#513](https://github.com/ligate-io/ligate-chain/issues/513): this runbook's tracking issue
- [chain#360](https://github.com/ligate-io/ligate-chain/issues/360): pre-mainnet security gap tracker
