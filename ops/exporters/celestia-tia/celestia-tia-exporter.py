#!/usr/bin/env python3
"""Tiny Prometheus exporter for the chain's DA-signer TIA balance + readiness.

Why this exists.

Celestia-node v0.30.x ships OTLP-only metrics. The chain's alert rules
in `ops/prometheus/alerts.yaml` (`TIABalanceLow`, `TIABalanceCritical`,
`TIADrawdownSpike`) reference `celestia_node_da_signer_balance_utia`,
a name that doesn't exist anywhere by default. This exporter exposes
that metric on `:9101/metrics` by polling the celestia-light JSON-RPC.

It deliberately queries `state.BalanceForAddress` with the
chain-recorded `seq_da_address` (the address actually submitting
blobs), NOT `state.Balance` (which returns the light node's default
account, often a different address that holds nothing).

It also probes the chain's REST `/v1/health/ready` and exposes a
`ligate_ready` gauge so the `ReadyFalse` alert in alerts.yaml has a
metric to fire on. Without this, the alert sits silent (blackbox
exporter isn't deployed). One process for the two thinnest metrics we
own keeps the systemd unit count down on the sequencer VM.

A dead-man's switch metric (`celestia_tia_exporter_alive`) flips to 0
if no balance poll has succeeded in DEAD_MANS_SWITCH_SEC (default
300s). Alert rules can fire on either the balance being stale OR the
exporter being dead, separately from balance going low.

Config via env:
    LIGHT_NODE_STORE        /home/ligate/.celestia-light-mocha-4
    LIGHT_NODE_RPC          http://127.0.0.1:26658
    LIGHT_NODE_NETWORK      mocha
    DA_SIGNER_ADDRESS       celestia1mphanjz38a5fvftsdr20ct388gtnxhlq52wr33
    DA_SIGNER_ACCOUNT       da-signer   (friendly label; defaults to "da-signer")
    LIGATE_INSTANCE         ligate-devnet-1-sequencer
    LIGATE_READY_URL        http://127.0.0.1:12346/ready
    LIGATE_READY_TIMEOUT    3   (seconds)
    POLL_INTERVAL_SEC       60
    DEAD_MANS_SWITCH_SEC    300
    EXPORTER_BIND           0.0.0.0:9101

Auth token: minted on startup via `celestia light auth admin` and
re-minted on 401 errors. The token never expires by default (Celestia
issues with ExpiresAt = zero-value), so this is mostly belt-and-braces.

Dependencies: stdlib only. Runs under Python 3.10+.
"""

from __future__ import annotations

import http.server
import json
import logging
import os
import re
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone

# ----- Config ----------------------------------------------------------------

LIGHT_STORE = os.environ.get("LIGHT_NODE_STORE", "/home/ligate/.celestia-light-mocha-4")
LIGHT_RPC = os.environ.get("LIGHT_NODE_RPC", "http://127.0.0.1:26658")
LIGHT_NETWORK = os.environ.get("LIGHT_NODE_NETWORK", "mocha")
DA_SIGNER_ADDRESS = os.environ["DA_SIGNER_ADDRESS"]  # required; no default
# Friendly per-account label. Useful once we run a second signer (rotation,
# hot/cold split, etc.) so dashboards group by account name instead of by
# a 45-char bech32. Currently always "da-signer".
DA_SIGNER_ACCOUNT = os.environ.get("DA_SIGNER_ACCOUNT", "da-signer")
INSTANCE = os.environ.get("LIGATE_INSTANCE", "ligate-devnet-1-sequencer")
LIGATE_READY_URL = os.environ.get("LIGATE_READY_URL", "http://127.0.0.1:12346/ready")
LIGATE_READY_TIMEOUT = float(os.environ.get("LIGATE_READY_TIMEOUT", "3"))
POLL_INTERVAL_SEC = int(os.environ.get("POLL_INTERVAL_SEC", "60"))
# Dead-man's switch threshold: how long the last successful balance poll
# can be stale before `celestia_tia_exporter_alive` flips to 0. Default
# is 5x the default poll interval to absorb transient celestia-light
# blips without alerting.
DEAD_MANS_SWITCH_SEC = int(os.environ.get("DEAD_MANS_SWITCH_SEC", "300"))
EXPORTER_BIND = os.environ.get("EXPORTER_BIND", "0.0.0.0:9101")

logging.basicConfig(
    format="%(asctime)s %(levelname)s %(message)s",
    level=logging.INFO,
    stream=sys.stderr,
)
log = logging.getLogger("celestia-tia-exporter")

# ----- Shared state (atomic-ish via GIL; bool/int writes are atomic) --------

_state_lock = threading.Lock()
_state: dict = {
    "balance_utia": None,        # int | None
    "last_success_ts": None,     # epoch seconds, when last balance poll succeeded
    "last_error": None,          # str | None
    "consecutive_failures": 0,
    "token": None,               # cached JWT
    "ligate_ready": None,        # int (1/0) | None — last /v1/health/ready probe outcome
    "ligate_ready_last_check_ts": None,  # epoch seconds, when last ready probe finished (success or fail)
    "ligate_ready_last_error": None,     # str | None
}


# ----- Celestia helpers ------------------------------------------------------

def mint_auth_token() -> str:
    """Run `celestia light auth admin` to mint a fresh JWT.

    Output of that command is a multi-line blob; we grep the JWT
    pattern out of it. Runs as the current process user (must own
    the keystore under LIGHT_STORE).
    """
    proc = subprocess.run(
        [
            "celestia",
            "light",
            "auth",
            "admin",
            "--node.store",
            LIGHT_STORE,
            "--p2p.network",
            LIGHT_NETWORK,
        ],
        capture_output=True,
        text=True,
        timeout=10,
    )
    combined = proc.stdout + "\n" + proc.stderr
    match = re.search(r"eyJ[A-Za-z0-9_.-]{50,}", combined)
    if not match:
        raise RuntimeError(
            f"celestia auth output had no JWT match; rc={proc.returncode}\n"
            f"stdout: {proc.stdout[:200]}\nstderr: {proc.stderr[:200]}"
        )
    return match.group(0)


def rpc_call(method: str, params: list, token: str) -> dict:
    """Issue a single JSON-RPC call against the local celestia-light."""
    payload = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    req = urllib.request.Request(
        LIGHT_RPC,
        data=payload,
        headers={
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json",
        },
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=10) as resp:
        body = resp.read().decode()
    parsed = json.loads(body)
    if "error" in parsed:
        raise RuntimeError(f"RPC error: {parsed['error']}")
    return parsed.get("result", {})


def fetch_balance() -> int:
    """Return the DA signer's TIA balance in utia (micro-TIA)."""
    with _state_lock:
        token = _state["token"]
    if not token:
        token = mint_auth_token()
        with _state_lock:
            _state["token"] = token

    try:
        result = rpc_call("state.BalanceForAddress", [DA_SIGNER_ADDRESS], token)
    except urllib.error.HTTPError as e:
        if e.code == 401:
            # Token rotated / wiped; mint a fresh one and retry once.
            log.warning("auth 401; re-minting token")
            token = mint_auth_token()
            with _state_lock:
                _state["token"] = token
            result = rpc_call("state.BalanceForAddress", [DA_SIGNER_ADDRESS], token)
        else:
            raise

    denom = result.get("denom", "")
    amount = result.get("amount", "0")
    if denom != "utia":
        log.warning("unexpected denom=%s; expected utia. raw: %s", denom, result)
    return int(amount)


# ----- Ligate-node readiness probe ------------------------------------------

def probe_ligate_ready() -> int:
    """Hit `LIGATE_READY_URL` (defaults to chain's `/ready`) and return 1 if 2xx, 0 otherwise.

    Treats both HTTP errors and connection errors as "not ready". The
    HTTP error path covers cases where the chain is up but stuck (5xx
    from a health handler that knows it's degraded).

    Path note: the chain exposes `/ready` and `/health` at the root (no
    `/v1/` prefix); both are wired through Caddy. `/v1/healthcheck` also
    works but returns a literal `ok` rather than a JSON body. The
    earlier v0.2.1 cut shipped this with a wrong default URL
    (`/v1/health/ready`, which 404s).
    """
    req = urllib.request.Request(LIGATE_READY_URL, method="GET")
    try:
        with urllib.request.urlopen(req, timeout=LIGATE_READY_TIMEOUT) as resp:
            return 1 if 200 <= resp.status < 300 else 0
    except urllib.error.HTTPError as e:
        # 4xx/5xx from the health endpoint = explicitly not ready.
        return 1 if 200 <= e.code < 300 else 0


# ----- Poller loop -----------------------------------------------------------

def poll_loop() -> None:
    while True:
        # --- TIA balance ---
        try:
            balance = fetch_balance()
            now_ts = int(time.time())
            with _state_lock:
                _state["balance_utia"] = balance
                _state["last_success_ts"] = now_ts
                _state["last_error"] = None
                _state["consecutive_failures"] = 0
            log.info("poll ok: balance_utia=%d (%.6f TIA)", balance, balance / 1e6)
        except Exception as exc:
            with _state_lock:
                _state["last_error"] = str(exc)
                _state["consecutive_failures"] += 1
            log.error("poll failed (%d consecutive): %s",
                      _state["consecutive_failures"], exc)

        # --- Ligate-node readiness ---
        # Cheap: one localhost GET. Done in-line so both metrics share a
        # single cadence + a single sleep below.
        try:
            ready = probe_ligate_ready()
            with _state_lock:
                _state["ligate_ready"] = ready
                _state["ligate_ready_last_check_ts"] = int(time.time())
                _state["ligate_ready_last_error"] = None
            log.info("ready probe: ligate_ready=%d", ready)
        except Exception as exc:
            with _state_lock:
                _state["ligate_ready"] = 0
                _state["ligate_ready_last_check_ts"] = int(time.time())
                _state["ligate_ready_last_error"] = str(exc)
            log.warning("ready probe failed: %s", exc)

        time.sleep(POLL_INTERVAL_SEC)


# ----- HTTP exposition -------------------------------------------------------

class MetricsHandler(http.server.BaseHTTPRequestHandler):
    def log_message(self, format, *args):
        # Quiet the default access log; we have our own logger.
        return

    def do_GET(self) -> None:
        if self.path != "/metrics":
            self.send_response(404)
            self.end_headers()
            self.wfile.write(b"not found\n")
            return

        with _state_lock:
            snapshot = dict(_state)

        out_lines: list[str] = []

        # ----- Balance gauge (the metric alerts.yaml fires on) --------------
        out_lines.append(
            "# HELP celestia_node_da_signer_balance_utia "
            "DA signer wallet balance in utia (micro-TIA) as observed by polling state.BalanceForAddress against the local light node."
        )
        out_lines.append("# TYPE celestia_node_da_signer_balance_utia gauge")
        if snapshot["balance_utia"] is not None:
            out_lines.append(
                f'celestia_node_da_signer_balance_utia{{instance="{INSTANCE}",address="{DA_SIGNER_ADDRESS}",account="{DA_SIGNER_ACCOUNT}"}} {snapshot["balance_utia"]}'
            )

        # ----- Per-poll diagnostics -----------------------------------------
        out_lines.append(
            "# HELP celestia_tia_exporter_last_success_seconds "
            "Unix timestamp of the last successful poll of the DA-signer balance. 0 if no successful poll yet."
        )
        out_lines.append("# TYPE celestia_tia_exporter_last_success_seconds gauge")
        out_lines.append(
            f'celestia_tia_exporter_last_success_seconds{{instance="{INSTANCE}",account="{DA_SIGNER_ACCOUNT}"}} {snapshot["last_success_ts"] or 0}'
        )

        out_lines.append(
            "# HELP celestia_tia_exporter_consecutive_failures "
            "Count of consecutive failed polls since the last success. Reset to 0 on success."
        )
        out_lines.append("# TYPE celestia_tia_exporter_consecutive_failures gauge")
        out_lines.append(
            f'celestia_tia_exporter_consecutive_failures{{instance="{INSTANCE}",account="{DA_SIGNER_ACCOUNT}"}} {snapshot["consecutive_failures"]}'
        )

        # ----- Dead-man's switch --------------------------------------------
        # 1 if a balance poll succeeded within DEAD_MANS_SWITCH_SEC, else 0.
        # Lets alert rules distinguish "balance is low" from "we have no idea
        # what the balance is" — the latter usually means the celestia-light
        # node is dead, not the wallet.
        now_ts = int(time.time())
        last_ok = snapshot["last_success_ts"]
        alive = 1 if (last_ok is not None and now_ts - last_ok <= DEAD_MANS_SWITCH_SEC) else 0
        out_lines.append(
            "# HELP celestia_tia_exporter_alive "
            "Dead-man's switch: 1 if a balance poll succeeded within DEAD_MANS_SWITCH_SEC, else 0."
        )
        out_lines.append("# TYPE celestia_tia_exporter_alive gauge")
        out_lines.append(
            f'celestia_tia_exporter_alive{{instance="{INSTANCE}",account="{DA_SIGNER_ACCOUNT}"}} {alive}'
        )

        # ----- Chain readiness probe ----------------------------------------
        # Emits 1 if the chain's /v1/health/ready returned 2xx on the last
        # probe, 0 otherwise. Silent absence (no line) before the first probe
        # completes so the ReadyFalse alert doesn't fire from cold start.
        if snapshot["ligate_ready"] is not None:
            out_lines.append(
                "# HELP ligate_ready "
                "1 if /v1/health/ready returned 2xx on the last probe, 0 otherwise. Probed by celestia-tia-exporter every POLL_INTERVAL_SEC."
            )
            out_lines.append("# TYPE ligate_ready gauge")
            out_lines.append(
                f'ligate_ready{{instance="{INSTANCE}"}} {snapshot["ligate_ready"]}'
            )

            out_lines.append(
                "# HELP ligate_ready_last_check_seconds "
                "Unix timestamp of the last /v1/health/ready probe (success OR fail). 0 if never probed."
            )
            out_lines.append("# TYPE ligate_ready_last_check_seconds gauge")
            out_lines.append(
                f'ligate_ready_last_check_seconds{{instance="{INSTANCE}"}} {snapshot["ligate_ready_last_check_ts"] or 0}'
            )

        body = "\n".join(out_lines) + "\n"
        self.send_response(200)
        self.send_header("Content-Type", "text/plain; version=0.0.4; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body.encode())


def main() -> None:
    host, port_str = EXPORTER_BIND.rsplit(":", 1)
    port = int(port_str)

    log.info(
        "starting celestia-tia-exporter: bind=%s rpc=%s store=%s signer=%s poll=%ds",
        EXPORTER_BIND, LIGHT_RPC, LIGHT_STORE, DA_SIGNER_ADDRESS, POLL_INTERVAL_SEC,
    )
    log.info("started at %s UTC", datetime.now(timezone.utc).isoformat(timespec="seconds"))

    poller = threading.Thread(target=poll_loop, daemon=True, name="poll-loop")
    poller.start()

    server = http.server.ThreadingHTTPServer((host, port), MetricsHandler)
    log.info("metrics server listening at http://%s%s/metrics", host, ":" + port_str)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        log.info("shutting down")
        server.server_close()


if __name__ == "__main__":
    main()
