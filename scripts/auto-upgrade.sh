#!/usr/bin/env bash
# Auto-upgrade poller for `ligate-node`. Runs hourly via the
# `ligate-auto-upgrade.timer` systemd unit. Safely swaps the chain
# binary when ALL of the following hold:
#
#   1. A new release is published on GitHub.
#   2. The release ships a `chain-hash-unchanged.flag` asset (set by
#      `release.yml` when the matching CHANGELOG section asserts
#      `chain_hash` is unchanged — the only case where an in-place
#      binary swap is safe without state quarantine + replay).
#   3. The new version isn't a major-version bump from the running
#      one (e.g. 0.x → 1.x stays manual).
#   4. `/etc/ligate/auto-upgrade.disabled` doesn't exist (operator
#      kill-switch: `touch` this file to pause auto-upgrades during
#      incident response without disabling the timer).
#
# Anything that fails the gates is a no-op log line; nothing destructive
# happens implicitly. The actual binary swap delegates to
# `upgrade-chain.sh` (snapshot → stop → swap → start → verify →
# auto-rollback on failure), so the safety story matches the manual
# upgrade path exactly.
#
# Tracking issue: ligate-io/ligate-chain#503.

set -euo pipefail

# ──────────────────────────────────────────────────────────────────────
# Tunables. Override via env if a non-default install is in use.
# ──────────────────────────────────────────────────────────────────────
KILL_SWITCH="${KILL_SWITCH:-/etc/ligate/auto-upgrade.disabled}"
LOCK_FILE="${LOCK_FILE:-/var/lock/ligate-auto-upgrade.lock}"
UPGRADE_SCRIPT="${UPGRADE_SCRIPT:-/opt/ligate/scripts/upgrade-chain.sh}"
RPC_URL="${RPC_URL:-http://127.0.0.1:12346}"
GH_OWNER_REPO="${GH_OWNER_REPO:-ligate-io/ligate-chain}"
SAFE_FLAG_NAME="${SAFE_FLAG_NAME:-chain-hash-unchanged.flag}"

# ──────────────────────────────────────────────────────────────────────
# Helpers.
# ──────────────────────────────────────────────────────────────────────
log() { printf '[auto-upgrade] %s\n' "$*"; }
warn() { printf '[auto-upgrade] WARNING: %s\n' "$*" >&2; }

# Bail without an error code on "nothing to do" exits. The systemd
# unit logs these to journald either way, but the timer doesn't go red
# on the next tick.
done_ok() { log "$*"; exit 0; }

# ──────────────────────────────────────────────────────────────────────
# Gate 1: kill switch.
# ──────────────────────────────────────────────────────────────────────
if [[ -f "$KILL_SWITCH" ]]; then
    done_ok "$KILL_SWITCH present; auto-upgrade paused. \`rm $KILL_SWITCH\` to re-enable."
fi

# ──────────────────────────────────────────────────────────────────────
# Concurrency. flock makes overlapping timer ticks safe (also covers
# operator running `auto-upgrade.sh` manually while the timer fires).
# ──────────────────────────────────────────────────────────────────────
exec 200>"$LOCK_FILE"
if ! flock -n 200; then
    done_ok "another auto-upgrade in flight (lock held); exiting"
fi

# ──────────────────────────────────────────────────────────────────────
# Read the running version off the chain itself. If the chain is
# down, defer — restarting a downed chain via an auto-swap risks
# masking a real outage. The hourly timer retries naturally.
# ──────────────────────────────────────────────────────────────────────
if ! INFO=$(curl -fsSL --max-time 5 "$RPC_URL/v1/rollup/info" 2>&1); then
    warn "chain RPC unreachable at $RPC_URL/v1/rollup/info; deferring"
    exit 0
fi
RUNNING=$(echo "$INFO" | jq -r '.version // empty')
if [[ -z "$RUNNING" ]]; then
    warn "couldn't parse .version from /v1/rollup/info response: $INFO"
    exit 0
fi
log "running version: v$RUNNING"

# ──────────────────────────────────────────────────────────────────────
# Look up the latest release.
# ──────────────────────────────────────────────────────────────────────
if ! RELEASE_JSON=$(curl -fsSL --max-time 10 \
        -H "Accept: application/vnd.github+json" \
        "https://api.github.com/repos/$GH_OWNER_REPO/releases/latest"); then
    warn "GitHub API unreachable; deferring"
    exit 0
fi
TAG=$(echo "$RELEASE_JSON" | jq -r '.tag_name // empty')
LATEST="${TAG#v}"
if [[ -z "$LATEST" ]]; then
    warn "couldn't parse .tag_name from latest release response"
    exit 0
fi

if [[ "$RUNNING" == "$LATEST" ]]; then
    done_ok "already at latest: v$LATEST"
fi
log "candidate upgrade: v$RUNNING → v$LATEST"

# ──────────────────────────────────────────────────────────────────────
# Gate 2: major-version safety. Major bumps stay manual indefinitely;
# they always imply something interesting (chain_hash, state shape,
# or genesis schema) that wants an operator in the loop.
# ──────────────────────────────────────────────────────────────────────
RUN_MAJOR="${RUNNING%%.*}"
NEW_MAJOR="${LATEST%%.*}"
if [[ "$RUN_MAJOR" != "$NEW_MAJOR" ]]; then
    done_ok "v$RUNNING → v$LATEST is a MAJOR bump; manual upgrade required"
fi

# ──────────────────────────────────────────────────────────────────────
# Gate 3: the chain-hash-unchanged.flag asset. Absent means the release
# author didn't assert chain_hash is preserved — i.e. either it did
# change, or the assertion got lost. Either way, manual.
# ──────────────────────────────────────────────────────────────────────
HAS_FLAG=$(echo "$RELEASE_JSON" | jq -r --arg n "$SAFE_FLAG_NAME" \
    '.assets[]? | select(.name == $n) | .name')
if [[ -z "$HAS_FLAG" ]]; then
    done_ok "v$LATEST is missing $SAFE_FLAG_NAME asset; treating as chain_hash-changing; manual upgrade required"
fi

# ──────────────────────────────────────────────────────────────────────
# All gates passed. Execute the upgrade through the canonical script;
# anything that goes wrong there (sha256 mismatch, service stays down,
# RPC reports the wrong version) auto-rollbacks to .prev and exits
# non-zero, which surfaces in journald and the next timer tick re-tries.
# ──────────────────────────────────────────────────────────────────────
log "all gates passed; invoking $UPGRADE_SCRIPT $LATEST"
"$UPGRADE_SCRIPT" "$LATEST"
log "auto-upgrade complete: v$LATEST is now serving"
