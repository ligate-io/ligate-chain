#!/usr/bin/env python3
"""Tiny Prometheus exporter for the chain's DA-signer TIA balance.

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

Config via env:
    LIGHT_NODE_STORE       /home/ligate/.celestia-light-mocha-4
    LIGHT_NODE_RPC         http://127.0.0.1:26658
    LIGHT_NODE_NETWORK     mocha
    DA_SIGNER_ADDRESS      celestia1mphanjz38a5fvftsdr20ct388gtnxhlq52wr33
    LIGATE_INSTANCE        ligate-devnet-1-sequencer
    POLL_INTERVAL_SEC      60
    EXPORTER_BIND          0.0.0.0:9101

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
INSTANCE = os.environ.get("LIGATE_INSTANCE", "ligate-devnet-1-sequencer")
POLL_INTERVAL_SEC = int(os.environ.get("POLL_INTERVAL_SEC", "60"))
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
    "last_success_ts": None,     # epoch seconds, when last poll succeeded
    "last_error": None,          # str | None
    "consecutive_failures": 0,
    "token": None,               # cached JWT
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


# ----- Poller loop -----------------------------------------------------------

def poll_loop() -> None:
    while True:
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

        out_lines.append(
            "# HELP celestia_node_da_signer_balance_utia "
            "DA signer wallet balance in utia (micro-TIA) as observed by polling state.BalanceForAddress against the local light node."
        )
        out_lines.append("# TYPE celestia_node_da_signer_balance_utia gauge")
        if snapshot["balance_utia"] is not None:
            out_lines.append(
                f'celestia_node_da_signer_balance_utia{{instance="{INSTANCE}",address="{DA_SIGNER_ADDRESS}"}} {snapshot["balance_utia"]}'
            )

        out_lines.append(
            "# HELP celestia_tia_exporter_last_success_seconds "
            "Unix timestamp of the last successful poll of the DA-signer balance. 0 if no successful poll yet."
        )
        out_lines.append("# TYPE celestia_tia_exporter_last_success_seconds gauge")
        out_lines.append(
            f'celestia_tia_exporter_last_success_seconds{{instance="{INSTANCE}"}} {snapshot["last_success_ts"] or 0}'
        )

        out_lines.append(
            "# HELP celestia_tia_exporter_consecutive_failures "
            "Count of consecutive failed polls since the last success. Reset to 0 on success."
        )
        out_lines.append("# TYPE celestia_tia_exporter_consecutive_failures gauge")
        out_lines.append(
            f'celestia_tia_exporter_consecutive_failures{{instance="{INSTANCE}"}} {snapshot["consecutive_failures"]}'
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
