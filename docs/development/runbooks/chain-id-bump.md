# Runbook — Chain-id bump (pre-mainnet hard fork)

**Status:** v0 — covers `ligate-devnet-1 → ligate-devnet-2` and the analogous testnet rotation. Mainnet-grade comms are out of scope; the upgrade-module ceremony ([#42](https://github.com/ligate-io/ligate-chain/issues/42)) supersedes this runbook post-mainnet. Companion to [`upgrades.md`](../../protocol/upgrades.md), the policy doc this operationalises.

A chain-id bump is the pre-mainnet hard-fork mechanism: state is dropped, a fresh genesis is published, every operator restarts against the new chain. State carries over only via off-chain re-registration (partners re-fund dev keys, re-submit schema registrations, etc.).

The trailing number on the chain-id ladder is the only thing that moves: `ligate-localnet → unaffected`, `ligate-devnet-1 → ligate-devnet-2`, `ligate-testnet-1 → ligate-testnet-2`, `ligate-1 → ligate-2`. See [`upgrades.md` "Hard fork"](../../protocol/upgrades.md#what-counts-as-a-hard-fork) for what changes warrant this.

---

## Decision gate (T-2 weeks)

Before scheduling a bump, confirm the change is genuinely hard-fork class:

- [ ] Borsh snapshot fires on the proposed change (run `cargo test -p attestation borsh_snapshot`)
- [ ] OR `CHAIN_HASH` shifts (check the build output of `cargo build -p ligate-stf`)
- [ ] OR a `MAX_*` constant changes (`MAX_ATTESTOR_SET_MEMBERS`, `MAX_ATTESTATION_SIGNATURES`, etc.)
- [ ] OR runtime composition changes (module added / removed)

If none of these fire, it's a soft fork. Ship through normal release channels and skip this runbook.

---

## Re-genesis variants (and the DA-replay gotcha)

This runbook's main flow assumes a **chain-id bump** (`ligate-devnet-N → ligate-devnet-N+1`): fresh genesis, new DA namespace, no shared history. That's the clean cut.

You may also need a **re-genesis WITHOUT a chain-id bump** if you ship a wire-format change that requires fresh state but you want to keep the network identity. The v0.2.0 cut on 2026-05-17 was this case: `AttestationId` collapsed from compound `<schema_id>:<payload_hash>` to single `lat1...` bech32m; on-chain state was wiped but `chain_id` stayed `ligate-devnet-1`.

When you re-genesis with the SAME `chain_id` AND the same DA namespace:

- **The chain replays all historical Celestia blobs on next boot.** They're immutable on Mocha; you can't unsubmit them. The new binary processes them under the new code paths and reconstructs equivalent state (with the new wire format). For the v0.2.0 cut this was actually a clean outcome: all 4 schemas + 2 attestor sets + 8 attestations re-emerged as `lat1...`-keyed records without re-running the bootstrap ceremony.
- **`chain_hash` may or may not shift**, depending on whether the genesis state-root computation depends on the changed type identity. v0.2.0 left `chain_hash` unchanged (genesis was empty for the affected maps, so the state-tree root was identical). Verify post-restart: `curl https://rpc.ligate.io/v1/rollup/info | jq .chain_hash` and compare against the docs/`constants.toml` pin.
- **To get a TRULY clean restart with no historical replay**, either:
  - **Advance `genesis_da_height`** in `devnet-1/celestia.toml` past the prior chain's last-submitted blob (find it via the Celestia explorer for the `lig-dvn-tb` namespace). The chain then skips reading older blobs.
  - **OR rotate to a fresh DA namespace** (e.g. `lig-dvn-tb-2`). New namespace = no historical blobs to replay.

If you don't take one of those steps and you DO want clean state, the DA replay will reconstruct the old activity under the new code. Treat that as expected, not a bug.

---

## Pre-bump checklist (T-7 days)

### Engineering

- [ ] Issue filed describing the wire-format / runtime-composition change requiring the bump. Linked to this runbook execution.
- [ ] PR with the breaking change merged to a `bump-target` branch (NOT `main`).
- [ ] New `CHAIN_HASH` pinned in `crates/stf/src/chain_hash_pin.rs`. Verified deterministic across two clean checkouts.
- [ ] New genesis files prepared at `localnet/genesis-${NEW_CHAIN_ID}/`. Includes:
  - Updated `chain_id` in `genesis.json`
  - Same sequencer registry as the outgoing chain (unless rotating per [`sequencer-key-rotation.md`](./sequencer-key-rotation.md))
  - Canonical Themisra schemas (via `ligate-bootstrap` binary, or `cargo run -p ligate-bootstrap-cli`, if re-registering)
- [ ] `localnet/rollup.toml` updated to point at the new genesis dir and chain id
- [ ] **Faucet/drip address funded in genesis.** `devnet/genesis/bank.json` MUST include the `ligate-api` drip address (`lig1kfaqfux…`) with a balance (1M AVOW). A fresh genesis otherwise leaves it at zero, and `ligate-api`'s startup drip-budget check (`GET /v1/modules/bank/tokens/{AVOW}/balances/{drip}`) 404s on a zero-balance address → the api **refuses to boot** → every Railway deploy crash-loops → the public API silently stays pinned to the pre-cutover deployment and indexes nothing on the new chain, even though the chain itself is healthy. Genesis balances do not affect `chain_hash`. (Baked into the committed `bank.json` as of the devnet-3 cutover; verify it's present before bumping. Emergency unblock if you forgot: set `DRIP_MIN_BUDGET=0` on the api to skip the check, then fund the drip address.)

### Operations

- [ ] Cutover datetime decided. Default: 14:00 UTC on a Tuesday (mid-week, mid-day across NA/EU; avoids weekend on-call for the operator).
- [ ] Status banner draft prepared for `docs.ligate.io` + `ligate.io` ("`ligate-devnet-N` ends 2026-XX-XX 14:00 UTC, `ligate-devnet-N+1` resumes immediately after").
- [ ] Partner notification list pulled from Attio (filter: status = devnet-active).
- [ ] Faucet TIA balance verified ≥30 days of expected burn on the new chain (per [`da-signer-rotation.md` "TIA balance monitoring"](./da-signer-rotation.md#tia-balance-monitoring)).

### Templates ready

The templates live in [`templates/`](./templates/) alongside this runbook:

- `partner-notification-t7d.md` — T-7d announce
- `partner-notification-t24h.md` — T-24h reminder
- `partner-notification-t1h.md` — T-1h "starts soon"
- `status-thread.md` — Discord / X thread skeleton with placeholders for timeline + contacts
- `re-registration.sh` — single-command script partners run to re-fund their dev key + re-register schemas on the new chain

These templates aren't checked in yet — file them as a follow-up to this runbook's first execution; the actual T-7d announce text will be drafted in `~/Desktop/ligate-docs/chain/` and copy-pasted into the runbook templates after the first bump fires.

---

## Pre-bump checklist (T-24 hours)

- [ ] T-24h reminder broadcast (email + Discord pending) to partners. Template: `templates/partner-notification-t24h.md`.
- [ ] New genesis hash published. Two locations:
  - GitHub release on `ligate-io/ligate-chain` tagged `v0.1.0-devnet-N+1-genesis` (or appropriate)
  - Pinned in `docs.ligate.io/localnet/genesis` page
- [ ] Faucet pre-funded on the new chain id. Confirm by querying balance of the bootstrap address on the new genesis.
- [ ] Status banner live on `docs.ligate.io` + `ligate.io`.
- [ ] Indexer / explorer Postgres backup taken (`pg_dump` to GCS bucket). Restore-from-snapshot will not be needed but the rollback path is "restore Postgres, point indexer at old chain RPC archive" if the bump aborts.

---

## Cutover (T-0)

Times are wall-clock from the announced cutover datetime.

### T-0:00 — Halt the old chain

```sh
# On the sequencer host
systemctl stop ligate-node
```

Record the final block height in the status thread. Operators verify their last-known block matches.

### T-0:02 — Verify final height converged

Two independent operators (Stefan + one attestor org operator pre-devnet) confirm via local node:

```sh
curl -s http://127.0.0.1:12346/v1/ledger/slots/latest | jq .number
```

Both must agree on the final height before proceeding.

### T-0:05 — Deploy the new binary + genesis

```sh
# Swap in the new chain config
mv /opt/ligate/devnet-1 /opt/ligate/devnet-1-archived
ln -sf /opt/ligate/devnet-${N+1} /opt/ligate/devnet-current
# Update systemd unit to point at the new config dir (or rely on the
# symlink above). Restart.
systemctl daemon-reload
systemctl start ligate-node
```

### T-0:10 — First block on new chain

Watch the journal for the first block emit:

```sh
journalctl -fu ligate-node | grep "block_committed"
```

Two independent operators confirm the first new-chain block is identical between their local views (same hash, same state root).

### T-0:15 — Faucet + indexer resumption

- Faucet: confirm `curl https://faucet.ligate.io/health` returns the new `chain_id` field.
- Indexer: confirm `curl https://api.ligate.io/v1/info` returns the new `chain_id` and `indexer_height` advancing past 0.
- Explorer: spot-check `explorer.ligate.io` renders the new latest block.

**Update the api's signing env vars to the new chain's values, and do it BEFORE the api redeploy.** The faucet's drip signer binds every tx to `chain_hash` and transfers `AVOW_TOKEN_ID`, and both change on a re-genesis: `chain_hash` whenever the runtime composition shifts (e.g. a new module in a version bump), and the `AVOW` token id on every fresh genesis. If they stay stale the api boots fine and the GET surface looks healthy, but `POST /v1/drip` submits transactions that **fail signature authentication** on the new chain. The recipient is never funded, the drip handler's inclusion poll times out, and the caller gets a **502** with `drip/status` reporting `addresses_dripped: 0`. Set both (64-char hex) on the `ligate-api` service:

```sh
# chain_hash: decode the bech32m chain hash from the new chain's
#   `GET /v1/rollup/info` (`lsch1...`) to 32-byte hex.
# avow_token_id: decode the new AVOW token id (`token_1...`, e.g. from
#   any funded address's `GET /v1/addresses/<addr>`) to 32-byte hex.
# (ligate-js: `decodeChainHash` / `tokenIdToHex` produce the hex.)
railway variables \
  --set "CHAIN_HASH=<64-hex>" \
  --set "AVOW_TOKEN_ID=<64-hex>" \
  --service ligate-api
```

Discovered on the devnet-3 cutover: the faucet 502'd for hours because v0.4.0 added the bounty + contract modules (shifting `chain_hash`) and the fresh genesis minted a new `AVOW` token id, but the api kept the devnet-2 values. This is the second re-genesis faucet gotcha alongside genesis-funding the drip address above.

**If you re-genesised with the same `chain_id` (the v0.2.0 case): you MUST also wipe the api's Postgres + restart the api process.** The api's TRUNCATE migration only covers the `attestations` table; the other 6 tables (`slots`, `transactions`, `attestor_sets`, `schemas`, `address_summaries`, `indexer_state`) keep their pre-wipe rows, AND the api's in-memory cursor `indexer_state.last_indexed_height` caches the pre-wipe height. The api will silently stop ingesting because the chain has "regressed" below its cached cursor.

Recovery (do these in order):

```sh
# 1. Connect to the api's Railway Postgres (DATABASE_PUBLIC_URL from
#    `railway variable list --service Postgres`).
PGURL='postgresql://postgres:...@yamanote.proxy.rlwy.net:.../railway'

# 2. TRUNCATE every indexer-managed table. Keep `_sqlx_migrations`.
psql "$PGURL" -c "
  TRUNCATE TABLE slots, transactions, attestor_sets, schemas,
                 attestations, address_summaries, indexer_state
  RESTART IDENTITY CASCADE;
"

# 3. Force the api to redeploy so the in-memory cursor resets. The
#    new container will read an empty `indexer_state`, log
#    'fresh db; starting at slot 1', and re-ingest from genesis.
railway redeploy --service ligate-api --yes

# 4. Verify (within ~1 min of redeploy):
curl -s https://api.ligate.io/v1/info | jq
# expect: indexer_height growing from 0, head_lag_slots decreasing
```

Indexer catch-up rate is ~1.5 slots/sec at devnet load; full catch-up to a 30k-slot chain takes ~5-6 hours. Don't fold this into the announcement timeline; the chain itself is fine in the meantime, just the api dashboards lag.

If chain-id ALSO bumped (the canonical hard-fork path), the steps above are still required (a fresh schema migration isn't a substitute for wiping the pre-bump state).

### T-0:30 — Public announcement

- Status banner flipped to "`ligate-devnet-N+1` is live; partners running on `N` should follow the re-registration steps".
- Status thread updated with first-block hash + timestamp.
- Partner notification (template: T+15min "you can re-register now").

---

## Re-registration (partner-facing)

Partners run a single script to re-fund their dev key + re-register schemas:

```sh
# From any host that has ligate-cli installed:
curl -sSL https://raw.githubusercontent.com/ligate-io/ligate-chain/main/scripts/devnet-rebootstrap.sh \
  | bash -s -- --address $(ligate keys show first)
```

This script:
1. Reads the partner's address
2. POSTs to the faucet endpoint on the new chain (1 AVOW drip)
3. Re-submits each schema registration from a local `schemas.json` if present
4. Prints the new schema ids (deterministic; should match the old chain unless attestor-set membership changed)

The script lives at `scripts/devnet-rebootstrap.sh` ([open follow-up](#open-follow-ups) — not yet checked in).

---

## Post-bump (T+24 hours)

- [ ] Old chain RPC archived. Tag the final commit `archive/ligate-devnet-N-final`. Pin Postgres backup at `gs://ligate-archive/devnet-N-final.sql.gz`. Document the archive URL in the chain-id ladder page.
- [ ] Re-registration help channel staffed for 24h (Discord pending; until then, `hello@ligate.io`).
- [ ] Postmortem doc opened at `docs/postmortems/chain-id-bump-${DATE}.md`. Capture: how long the cutover took vs planned, what surprised us, runbook revisions.
- [ ] [`upgrades.md`](../../protocol/upgrades.md) "Pre-mainnet: coordinated redeploy" section updated with the new chain id added to the ladder example.

---

## Rollback (if the bump fails mid-cutover)

If the first new-chain block fails to land within 10 minutes of restart, or two operators disagree on the first-block hash:

1. **Stop the new chain** — `systemctl stop ligate-node` on the sequencer host.
2. **Restore the symlink** — `ln -sf /opt/ligate/devnet-1-archived /opt/ligate/devnet-current`.
3. **Restart on the old chain** — `systemctl start ligate-node`. Block height resumes from where it left off at T-0:00.
4. **Status banner update** — "`ligate-devnet-N+1` rollover aborted; investigating. `ligate-devnet-N` resumes."
5. **Postmortem opens immediately** — what failed, what's needed before retry.

The rollback works because the old chain's data on disk is untouched by the cutover. The old Celestia namespace also still has all the historical blobs.

---

## What this runbook deliberately doesn't cover

- **State migration tooling** — `ligate-migrate` binary, schema-diff helpers, replay-from-DA support. Pre-mainnet hard forks drop state by design (per [`upgrades.md`](../../protocol/upgrades.md#what-counts-as-a-hard-fork)); migration tooling is post-mainnet work.
- **Upgrade-module ceremony** — coordinated runtime swap at a block height. Lands with [#42](https://github.com/ligate-io/ligate-chain/issues/42) post-mainnet; this runbook becomes obsolete for in-mainnet upgrades at that point.
- **Mainnet-grade comms** — multi-week T-N timeline, validator-set notification, exchange / wallet maintainer outreach. Pre-mainnet, the attestor org pool is small enough that informal coordination suffices.
- **Cross-DA migration** — if we also rotate the Celestia testnet (mocha → mocha-2) at the same time, that's a separate hard-fork dimension covered in [`da-layers.md`](../../protocol/da-layers.md).

---

## Open follow-ups

1. **Template drafts.** Four notification templates (T-7d, T-24h, T-1h, T+15min) and one Discord/X status thread skeleton. Live in `~/Desktop/ligate-docs/chain/` until the first bump fires, then copy-paste into `docs/development/runbooks/templates/`.
2. **`scripts/devnet-rebootstrap.sh`.** Single-command partner re-registration script. Test against `ligate-localnet` before the first real bump.
3. **First-execution feedback loop.** Run this runbook end-to-end on the first `ligate-devnet-1 → ligate-devnet-2` cutover; surprises feed back here as revisions.
4. **Discord channel.** Pending; the partner help channel referenced in T+24h doesn't exist yet. Until then, `hello@ligate.io` is the fallback.

---

## Cross-references

- [`upgrades.md`](../../protocol/upgrades.md) — the policy this runbook operationalises
- [`upgrades.md` "Chain-id ladder"](../../protocol/attestation-v0.md#chain-id) — the locked four-row sequence
- [`sequencer-key-rotation.md`](./sequencer-key-rotation.md) — companion runbook if the bump also rotates the sequencer's chain-side key
- [`da-signer-rotation.md`](./da-signer-rotation.md) — companion if rotating the Celestia signer
- [Issue #42](https://github.com/ligate-io/ligate-chain/issues/42) — post-mainnet upgrade module (out of scope here)
- [Issue #54](https://github.com/ligate-io/ligate-chain/issues/54) — chain-id ladder definition
