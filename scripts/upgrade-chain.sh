#!/usr/bin/env bash
# One-command in-place upgrade for `ligate-node` on a devnet VM.
#
# Usage:
#   upgrade-chain.sh <version>          # e.g. upgrade-chain.sh 0.2.13 (or v0.2.13)
#   upgrade-chain.sh rollback           # restore the previous binary saved on the last upgrade
#   upgrade-chain.sh --dry-run <version>  # print what would happen, exit
#
# Flow:
#   1. Resolve target version + arch + download URLs.
#   2. Fetch tarball + sha256 sidecar; verify integrity.
#   3. Snapshot the current binary to `<bin>.prev` so rollback is one cmd.
#   4. systemctl stop ligate-node (idempotent).
#   5. Install the new binary with the same owner / mode as the running one.
#   6. systemctl start ligate-node + poll until `active` (or auto-rollback).
#   7. Curl-check `/v1/rollup/info` reports the target version.
#
# If steps 6 or 7 fail, the script automatically swaps `.prev` back in
# and restarts, so a broken release never leaves devnet-1 in a stopped
# state. Manual rollback any time later via `upgrade-chain.sh rollback`.
#
# Tracking issue: ligate-io/ligate-chain#349.
# Cross-ref: docs/development/runbooks/chain-upgrade.md (procedure +
# pre-/post-checks).

set -euo pipefail

# ──────────────────────────────────────────────────────────────────────
# Tunables. Override via env if a non-default install is in use.
# ──────────────────────────────────────────────────────────────────────
LIGATE_BIN="${LIGATE_BIN:-/opt/ligate/bin/ligate-node}"
LIGATE_USER="${LIGATE_USER:-ligate}"
LIGATE_GROUP="${LIGATE_GROUP:-ligate}"
LIGATE_SERVICE="${LIGATE_SERVICE:-ligate-node}"
LIGATE_RPC_URL="${LIGATE_RPC_URL:-http://127.0.0.1:12346}"
GH_OWNER_REPO="${GH_OWNER_REPO:-ligate-io/ligate-chain}"
START_TIMEOUT_SECS="${START_TIMEOUT_SECS:-90}"
WORKDIR="${WORKDIR:-/tmp/ligate-upgrade}"

# ──────────────────────────────────────────────────────────────────────
# Helpers.
# ──────────────────────────────────────────────────────────────────────
log() { printf '\033[1;34m[upgrade-chain]\033[0m %s\n' "$*"; }
fail() { printf '\033[1;31m[upgrade-chain]\033[0m %s\n' "$*" >&2; exit 1; }

usage() {
    sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'
    exit 1
}

require_root_or_sudo() {
    if [[ $EUID -ne 0 ]] && ! sudo -n true 2>/dev/null; then
        fail "needs root (passwordless sudo or run as root)"
    fi
}

# Run `sudo` only when we're not already root. Some VMs run cloud-init
# as root and don't have sudo wired in the same way.
SUDO=""
[[ $EUID -ne 0 ]] && SUDO="sudo"

current_version() {
    # `ligate-node --version` prints other lines first (logging init);
    # the actual version is on the line containing `ligate-rollup` or
    # the exit-line. Tolerate both shapes.
    "$LIGATE_BIN" --version 2>&1 | grep -Eo 'ligate-rollup [0-9]+\.[0-9]+\.[0-9]+' | tail -1 | awk '{print $2}'
}

detect_arch() {
    case "$(uname -m)" in
        x86_64|amd64) echo "linux-amd64" ;;
        aarch64|arm64) echo "linux-arm64" ;;
        *) fail "unsupported arch: $(uname -m)" ;;
    esac
}

assert_target_release_exists() {
    local url="$1"
    local code
    code=$(curl -sS -o /dev/null -w '%{http_code}' -I -L "$url" || true)
    [[ "$code" == "200" ]] || fail "release URL not reachable (HTTP $code): $url"
}

# Wait for the service to reach `active` state. Returns 0 on active,
# 1 on timeout. Polls every 2s up to $START_TIMEOUT_SECS.
wait_for_active() {
    local elapsed=0
    while (( elapsed < START_TIMEOUT_SECS )); do
        local state
        state=$($SUDO systemctl is-active "$LIGATE_SERVICE" 2>&1 || true)
        if [[ "$state" == "active" ]]; then
            return 0
        fi
        sleep 2
        elapsed=$((elapsed + 2))
    done
    return 1
}

# Verify the RPC reports the target version. Tolerates a few seconds of
# warmup after `systemctl is-active` flips, since the HTTP listener can
# bind a tick after the service goes active.
verify_rpc_version() {
    local want="$1"
    local elapsed=0
    while (( elapsed < 30 )); do
        local got
        got=$(curl -sf "$LIGATE_RPC_URL/v1/rollup/info" 2>/dev/null \
              | python3 -c "import sys,json; print(json.load(sys.stdin).get('version',''))" 2>/dev/null || true)
        if [[ "$got" == "$want" ]]; then
            return 0
        fi
        sleep 2
        elapsed=$((elapsed + 2))
    done
    return 1
}

rollback_now() {
    log "ROLLBACK: restoring ${LIGATE_BIN}.prev"
    [[ -x "${LIGATE_BIN}.prev" ]] || fail "no ${LIGATE_BIN}.prev to roll back to"
    $SUDO install -m 0755 -o "$LIGATE_USER" -g "$LIGATE_GROUP" "${LIGATE_BIN}.prev" "$LIGATE_BIN"
    $SUDO systemctl restart "$LIGATE_SERVICE"
    if wait_for_active; then
        local v
        v=$(current_version)
        log "rollback complete; service active on ${v:-unknown}"
        return 0
    fi
    fail "rollback failed; check 'journalctl -u $LIGATE_SERVICE -n 50'"
}

# ──────────────────────────────────────────────────────────────────────
# Subcommand: rollback (manual).
# ──────────────────────────────────────────────────────────────────────
cmd_rollback() {
    require_root_or_sudo
    rollback_now
}

# ──────────────────────────────────────────────────────────────────────
# Subcommand: upgrade (default).
# ──────────────────────────────────────────────────────────────────────
cmd_upgrade() {
    local dry_run=0
    if [[ "${1:-}" == "--dry-run" ]]; then
        dry_run=1
        shift
    fi
    local version="${1:-}"
    [[ -n "$version" ]] || usage
    # Strip leading 'v' if present (tag form).
    version="${version#v}"

    local arch
    arch=$(detect_arch)
    local tarball="ligate-node-${version}-${arch}.tar.gz"
    local url="https://github.com/${GH_OWNER_REPO}/releases/download/v${version}/${tarball}"
    local sha_url="${url}.sha256"

    log "target:    v${version} (${arch})"
    log "from URL:  ${url}"
    log "service:   ${LIGATE_SERVICE}"
    log "binary:    ${LIGATE_BIN} (owner=${LIGATE_USER}:${LIGATE_GROUP})"
    log "rpc:       ${LIGATE_RPC_URL}"

    if (( dry_run )); then
        log "dry-run: nothing changed"
        exit 0
    fi

    require_root_or_sudo

    # Pre-flight: target binary exists on disk + service unit exists.
    [[ -x "$LIGATE_BIN" ]] || fail "current binary not found at $LIGATE_BIN"
    $SUDO systemctl cat "$LIGATE_SERVICE" >/dev/null 2>&1 || fail "service $LIGATE_SERVICE not configured"

    # Pre-flight: release tarball reachable before we touch anything.
    log "verifying release tarball is reachable..."
    assert_target_release_exists "$url"
    assert_target_release_exists "$sha_url"

    local from_version
    from_version=$(current_version || echo "unknown")
    log "current version: ${from_version}"
    if [[ "$from_version" == "$version" ]]; then
        log "already on v${version}; nothing to do"
        exit 0
    fi

    # Download + verify in a scratch dir.
    mkdir -p "$WORKDIR"
    cd "$WORKDIR"
    rm -rf "ligate-node-${version}-${arch}" "$tarball" "$tarball.sha256"

    log "downloading ${tarball}..."
    curl -fsSL -o "$tarball" "$url"
    curl -fsSL -o "$tarball.sha256" "$sha_url"

    log "verifying sha256..."
    sha256sum -c "$tarball.sha256"

    log "extracting..."
    tar -xzf "$tarball"

    local new_binary="${WORKDIR}/ligate-node-${version}-${arch}/ligate-node"
    [[ -x "$new_binary" ]] || fail "extracted archive is missing ligate-node binary"

    # Snapshot + swap. Stop first so the bytes-in-place change is safe.
    log "snapshotting current binary to ${LIGATE_BIN}.prev (rollback target)"
    $SUDO install -m 0755 -o "$LIGATE_USER" -g "$LIGATE_GROUP" "$LIGATE_BIN" "${LIGATE_BIN}.prev"

    log "stopping ${LIGATE_SERVICE}..."
    $SUDO systemctl stop "$LIGATE_SERVICE"

    log "installing new binary..."
    $SUDO install -m 0755 -o "$LIGATE_USER" -g "$LIGATE_GROUP" "$new_binary" "$LIGATE_BIN"

    log "starting ${LIGATE_SERVICE}..."
    $SUDO systemctl start "$LIGATE_SERVICE"

    log "waiting for service to reach 'active' (timeout ${START_TIMEOUT_SECS}s)..."
    if ! wait_for_active; then
        log "service did not reach 'active' in time"
        $SUDO journalctl -u "$LIGATE_SERVICE" -n 25 --no-pager >&2 || true
        rollback_now
        exit 1
    fi

    log "verifying RPC reports v${version}..."
    if ! verify_rpc_version "$version"; then
        log "RPC did not confirm v${version}"
        $SUDO journalctl -u "$LIGATE_SERVICE" -n 25 --no-pager >&2 || true
        rollback_now
        exit 1
    fi

    log "upgrade complete: ${from_version} → ${version} (service active)"
    log "rollback available any time via: $0 rollback"
}

# ──────────────────────────────────────────────────────────────────────
# Dispatch.
# ──────────────────────────────────────────────────────────────────────
case "${1:-}" in
    "" | -h | --help) usage ;;
    rollback) cmd_rollback ;;
    *) cmd_upgrade "$@" ;;
esac
