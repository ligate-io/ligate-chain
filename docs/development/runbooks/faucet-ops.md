# Runbook — Faucet operations

**Status:** v0 — operational layer for the devnet faucet (`faucet.ligate.io`). Companion to [#95](https://github.com/ligate-io/ligate-chain/issues/95), the faucet service implementation. Pre-public-devnet readiness: this runbook lands ahead of the faucet binary being live so the operator has the procedure ready before the first drip.

The faucet holds a hot key on a VM that signs `Transfer` txs to drip 1 $LGT to any address that asks (rate-limited). Three operational concerns flow from that:

1. The hot key gets compromised. PR problem; loss limited by the wallet's balance.
2. The wallet drains via legitimate usage or abuse. Service outage; partners can't onboard.
3. The treasury wallet that refills the faucet doesn't exist. The faucet can't be funded.

This runbook covers all three, plus rotation procedure modeled on [`sequencer-key-rotation.md`](./sequencer-key-rotation.md) and [`da-signer-rotation.md`](./da-signer-rotation.md).

---

## Architecture (one-screen recap)

```
                              treasury wallet (cold)
                              lig1xxx... (genesis-funded)
                                      │
                                      │ manual top-up (signed offline,
                                      │ broadcast via ligate-cli)
                                      ▼
        faucet hot wallet ── stores config + key on the VM at:
        lig1yyy...               /etc/ligate/faucet.key (mode 0600)
                                 │
                                 │ POST /faucet { address: lig1zzz... }
                                 ▼
        partner request ─────────► faucet service signs + submits
                                    a Transfer tx, returns tx hash
                                    + drip amount
```

Three keys, three trust scopes:
- **Genesis bootstrap** key (single use at genesis; provisions the treasury). Not used after devnet starts.
- **Treasury** key (cold; signing happens offline). Refills the faucet.
- **Faucet hot** key (on the VM; signs every drip).

Compromise blast radius scales with which key is exposed:
- Hot key leak → adversary drains faucet wallet. Treasury safe. Repaint faucet wallet, top up from treasury, ship.
- Treasury key leak → adversary moves the entire treasury balance. Worse, but treasury rotation is a real procedure (similar to [`sequencer-key-rotation.md`](./sequencer-key-rotation.md)).
- Genesis key was burned at genesis. Out of scope here.

---

## Key rotation runbook (planned)

Run periodically (every 90 days recommended pre-devnet) or immediately on compromise.

### 1. Generate the replacement hot wallet

```sh
# On a workstation, NOT the faucet VM:
ligate keys generate --name faucet-$(date +%Y%m%d)
# Records mnemonic. Back up immediately to the same place the treasury
# mnemonic lives (`~/Desktop/ligate-docs/chain/keys/` is the current
# convention).
```

The key file lives at `~/Library/Application Support/io.ligate.cli/keys/faucet-$DATE.key` (or `$XDG_DATA_HOME/ligate-cli/keys/` on Linux). Mode 0600. Never check into git.

### 2. Fund the new wallet from the treasury

```sh
# Sign offline from the treasury host. The treasury key never touches
# a network-connected machine.
ligate tx transfer \
  --from $TREASURY_ADDRESS \
  --to $(ligate keys show faucet-$(date +%Y%m%d)) \
  --amount $INITIAL_FAUCET_BALANCE_NANO \
  --offline \
  --output signed-tx.bin

# On the faucet host (or any host with internet), broadcast:
ligate tx broadcast --tx-file signed-tx.bin
```

`$INITIAL_FAUCET_BALANCE_NANO` is the balance to seed the new hot wallet with. Default: 30 days of expected burn (see [Balance monitoring](#balance-monitoring) below).

### 3. Stop the faucet service

```sh
# On the faucet VM
systemctl stop ligate-faucet
```

This is the only step that causes service interruption. Plan for ~2 minutes of downtime.

### 4. Swap the key file

```sh
# Backup the old key, install the new
cp /etc/ligate/faucet.key /etc/ligate/faucet.key.archived.$(date +%Y%m%d)
scp workstation:~/Library/Application\ Support/io.ligate.cli/keys/faucet-$DATE.key \
    /etc/ligate/faucet.key
chmod 0600 /etc/ligate/faucet.key
chown ligate:ligate /etc/ligate/faucet.key
```

### 5. Restart the service

```sh
systemctl start ligate-faucet
journalctl -fu ligate-faucet
# Wait for: "faucet ready; address: lig1..."
# Confirm the address matches the new wallet.
```

### 6. Drain the old wallet

```sh
# From a workstation, sweep the residual balance from the old faucet
# wallet back to the treasury. Use `--all` to leave only the tx fee:
ligate tx transfer \
  --from $OLD_FAUCET_ADDRESS \
  --to $TREASURY_ADDRESS \
  --all
```

Now the old key is no longer holding value. It can stay on the VM as a backup or be deleted.

### 7. Verify

```sh
# Hit the public endpoint
curl https://faucet.ligate.io/health | jq
# Expected: { "status": "ok", "address": "lig1...new..." }

curl -X POST https://faucet.ligate.io/faucet \
  -H "content-type: application/json" \
  -d '{"address":"lig1...test..."}'
# Expected: 200 with tx_hash; partner's balance increases by the
# drip amount.
```

**Estimated downtime:** ~2 minutes for the key swap + restart.

---

## Emergency rotation (under compromise)

Hot key is suspected leaked (exposed in a logged crash dump, accidental git commit, etc.). Same procedure as planned rotation but compressed and reordered:

1. **Drain the wallet FIRST** — before the adversary does. Sweep to treasury via the workstation:
   ```sh
   ligate tx transfer \
     --from $COMPROMISED_FAUCET_ADDRESS \
     --to $TREASURY_ADDRESS \
     --all
   ```
2. **Then rotate** — steps 1, 2, 3, 4, 5, 7 from the planned procedure. Skip step 6 (drain) because step 1 above did it.
3. **Update incident response doc** — what was the leak vector, when did we detect, what changes prevent recurrence (audit log retention shorter, secret-scan hooks on git, etc.).

Recovery time: ~10 minutes total if you have a pre-staged replacement key (recommended; keep one cold in `~/Desktop/ligate-docs/chain/keys/faucet-warm-spare.key`).

---

## Balance monitoring

The faucet wallet drains at a predictable rate via legitimate dripping. Without monitoring, it can hit zero silently (partners get 500s, file issues, we find out async).

### Threshold guidance

- **Drip amount:** 1 $LGT per request (configurable in the faucet service config)
- **Rate limit:** 1 drip per address per 24 hours
- **Expected daily burn (pre-devnet):** ~50 drips/day max during onboarding waves; lower at steady state. Plan for 100 drips/day headroom.
- **30-day burn:** ~3,000 $LGT
- **60-day burn:** ~6,000 $LGT

### Alert thresholds

| Threshold | Severity | Action |
|---|---|---|
| Balance < 60 days of expected burn (~6,000 $LGT) | Info | Plan a top-up within a week |
| Balance < 30 days (~3,000 $LGT) | Warn | Schedule top-up in the next 48h |
| Balance < 7 days (~700 $LGT) | Page | Top-up immediately; operator on-call |
| Balance == 0 | Critical | Service is down; emergency top-up + status banner |

### Manual check

```sh
ligate balance $FAUCET_ADDRESS --token-id $LGT_TOKEN_ID
# e.g. lig1...: 4,512.000000000 $LGT (4512000000000 nano)
```

### Prometheus metric (open follow-up)

The faucet service should expose a `ligate_faucet_balance_nano` gauge on its `/metrics` endpoint. The api crate's `/v1/info` endpoint could also surface this if we want it visible to partners. Alertmanager rule (track in [#268](https://github.com/ligate-io/ligate-chain/issues/268)):

```yaml
- alert: LigateFaucetLowBalance
  expr: ligate_faucet_balance_nano < 3_000_000_000_000  # 3000 $LGT in nano
  for: 5m
  annotations:
    runbook: docs/development/runbooks/faucet-ops.md#balance-monitoring
```

---

## Treasury top-up procedure

When the faucet balance triggers the 30-day-remaining alert (or earlier), refill from treasury.

```sh
# On the treasury host (air-gapped or hardened workstation)
ligate tx transfer \
  --from $TREASURY_ADDRESS \
  --to $FAUCET_ADDRESS \
  --amount $TOPUP_AMOUNT_NANO \
  --offline \
  --output topup-$(date +%Y%m%d).bin

# Move the signed tx to a network-connected host (USB transfer if air-
# gapped, otherwise just scp). Broadcast:
ligate tx broadcast --tx-file topup-$(date +%Y%m%d).bin

# Verify
ligate balance $FAUCET_ADDRESS --token-id $LGT_TOKEN_ID
```

**Top-up amount default:** 60 days of expected burn (~6,000 $LGT). Larger top-ups reduce operational frequency but increase exposure if the hot key is later compromised; smaller top-ups force more frequent refills. 60 days is the conservative compromise.

The treasury's own balance source is the genesis-funded bootstrap allocation. Treasury balance projection lives in `~/Desktop/ligate-docs/chain/economic-model.md` and should be updated whenever a major drip is made.

---

## Abuse mitigation

The rate limit (1 drip / address / 24h) at the application layer (in [#95](https://github.com/ligate-io/ligate-chain/issues/95)'s scope) catches casual abuse. What it doesn't catch:

- **Sybil farming** — adversary generates many fresh `lig1...` addresses and drips each. With 1 $LGT / address and ~1s to generate a fresh keypair, an attacker can drain ~3,000 $LGT/hour. At the 30-day burn threshold (3,000 $LGT), this is a 1-hour drain.
- **IP-level abuse** — same address farmed across multiple drips by exploiting the rate-limit's idempotency window.

### Escalation runbook

| Symptom | Response |
|---|---|
| Drip count spike >2x baseline over 1 hour | Operator notified via the same `LigateFaucetLowBalance` alert path (the abnormal drain rate trips low-balance earlier than expected). Watch the access logs (Caddy at `/var/log/caddy/access.log`). |
| Specific IP responsible | Add to `/etc/ligate/faucet-blocklist.txt` (one IP / CIDR per line), restart faucet service. The faucet reads this on startup; reload triggers a re-read. |
| Distributed sybil pattern | Raise rate-limit window from 24h to 48h. Reduce drip amount from 1 $LGT to 0.5. Both via `/etc/ligate/faucet.toml`, restart to apply. |
| Sustained abuse from multiple angles | Disable public faucet endpoint, point partners at an internal-only fallback (drip-by-request via `hello@ligate.io`). Status banner: "Faucet temporarily unavailable; email us for $LGT." |

### Long-term mitigations (out of scope here)

- **Captcha / hCaptcha gating.** Adds friction to legit users; tradeoffs documented when we revisit.
- **Proof-of-work gate** on drip requests. Effective but obnoxious for partners.
- **Onboarding-via-Discord** (drip only fires if the requester is in our Discord server). Defers to having a Discord server, which is pending.

---

## Chain of trust summary

| Key | Holder | Where stored | Used for | Rotation cadence |
|---|---|---|---|---|
| Genesis bootstrap | Stefan (offline) | `~/Desktop/ligate-docs/chain/keys/bootstrap.key` | Single use at genesis | One-shot; key burned post-genesis |
| Treasury | Stefan (offline) | `~/Desktop/ligate-docs/chain/keys/treasury.key` | Refilling faucet, future runtime payouts | 12 months or on compromise |
| Faucet hot | Faucet VM | `/etc/ligate/faucet.key` (mode 0600) | Signing drips | 90 days or on compromise |
| Sequencer chain | `ligate-node` VM | `/etc/ligate/sequencer.key` (mode 0600) | Signing batches | Per [`sequencer-key-rotation.md`](./sequencer-key-rotation.md) |
| DA signer | `ligate-node` VM | `rollup.toml` `[da].signer_private_key` | Signing Celestia blob txs | Per [`da-signer-rotation.md`](./da-signer-rotation.md) |

---

## Open follow-ups

1. **`ligate_faucet_balance_nano` metric** — wire from the faucet service to Prometheus. Pair with the Alertmanager rule above. Folds into [#268](https://github.com/ligate-io/ligate-chain/issues/268).
2. **Blocklist file format** — codify the schema for `/etc/ligate/faucet-blocklist.txt` (IP, CIDR, what about address-level blocks). Track as a follow-up to [#95](https://github.com/ligate-io/ligate-chain/issues/95).
3. **Top-up dry run** — execute the treasury top-up procedure once against `ligate-localnet` before public devnet announce, so the procedure isn't tested cold in production.
4. **Warm spare** — pre-generate a `faucet-warm-spare.key` and stash it in `~/Desktop/ligate-docs/chain/keys/`. Shrinks emergency rotation from ~10 min to ~2 min.

---

## Cross-references

- [Issue #95](https://github.com/ligate-io/ligate-chain/issues/95) — devnet faucet service (the implementation this runbook supports).
- [`sequencer-key-rotation.md`](./sequencer-key-rotation.md) — sibling runbook for the sequencer's chain-side key.
- [`da-signer-rotation.md`](./da-signer-rotation.md) — sibling runbook for the DA-side signer key.
- [Issue #268](https://github.com/ligate-io/ligate-chain/issues/268) — Alertmanager wiring; balance + blocklist alerts land there.
- [`public-devnet-deploy.md`](../public-devnet-deploy.md) — the deploy runbook this slots into ("Step 4.7: Faucet" section).
