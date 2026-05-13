# `HealthDown`

**Severity: page**

## What fired

Prometheus has been unable to scrape `ligate-node`'s `/metrics` endpoint for 30+ seconds.

Expression: `up{job="ligate-node"} == 0`

Faster signal than `BlockHeightStall` — the process is either down, panicked, or the metrics endpoint is wedged (the rest of the chain might still be functional).

## Likely causes

1. **Process crashed.** Panic, OOM kill, segfault, systemd restart loop.
2. **Process hung.** Metrics task deadlocked but the rest of the binary alive.
3. **Network partition** between Prometheus and the chain VM.
4. **Port binding lost** (very rare — could happen if a sibling process briefly grabbed `:9100`).

## First checks

```sh
# 1. Is the process even running?
systemctl status ligate-node
pgrep -lf ligate-node

# 2. Can the host reach the metrics port?
curl -sf http://127.0.0.1:9100/metrics | head -5

# 3. From the Prometheus host, can it reach the chain VM?
curl -sf http://<chain-vm-ip>:9100/metrics | head -5

# 4. systemd-reported status + last few log lines
journalctl -u ligate-node -n 50
```

## Fix

- **Crashed** → `systemctl restart ligate-node`. Cursor is persisted; resume is safe. File a bug with the panic stack.
- **Hung** → same: restart. If it hangs again within 5 minutes, capture a process stack (`sudo gcore <pid>` or `kill -QUIT <pid>` then read the SIGQUIT-handler core), file urgent bug, fall back to manual operation.
- **Network partition** → check the VM's external connectivity + the Prometheus host's firewall rules. Operator-side issue, not chain-side.
- **Port binding lost** → `ss -tlnp | grep 9100` to identify the squatter; kill it, restart `ligate-node`.

## False positives

- **Deliberate restart during a deploy.** Silence the alert for the deploy window or use Alertmanager's maintenance-mode feature.
- **Prometheus scrape interval longer than the down duration.** The alert's `for: 30s` should handle most cases; if Prometheus's scrape interval is 60s the alert can only confirm "down for at least one missed scrape, ~1-2min wall clock."

## Escalation

Pages immediately. If process won't restart cleanly, escalate to "single-operator can't recover alone" path (post in `#devnet-ops`, contact secondary operator if one exists, status banner up).
