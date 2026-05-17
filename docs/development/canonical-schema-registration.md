# Canonical schema registration runbook

How Ligate Labs registers `themisra.proof-of-prompt/v1` (and future first-party schemas) on the chain after genesis.

This is **post-genesis** work — runs once on a freshly-booted `ligate-devnet-1` (or any future devnet / mainnet boot), uses the same on-chain `RegisterAttestorSet` + `RegisterSchema` calls any third party would use, costs the registration fees the chain charges (`attestor_set_fee` 0.05 LGT + `schema_registration_fee` 0.1 LGT = ~0.15 LGT total per ceremony in v0). Fees in the genesis JSON are expressed in nano-LGT (9 decimals).

Tracking issue: [#231](https://github.com/ligate-io/ligate-chain/issues/231).

## Why post-genesis (not seeded at genesis)

`devnet-1/genesis/attestation.json` ships with `initial_attestor_sets: []` and `initial_schemas: []`. Themisra's first canonical schema gets registered by the `ligate-bootstrap` binary minutes after the chain boots, the same way a third-party schema would. Pros:

- The chain's "attestation primitive is permissionless from day one" promise stays honest — Themisra is no special case.
- No `payload_shape_hash: [0u8; 32]` placeholder permanently baked into chain state. The hash registered on-chain is the real SHA-256 of the canonical JSON Schema doc at `docs/protocol/schemas/themisra.proof-of-prompt-v1.json`.
- The schema spec (LIP-5) can iterate up until the moment of registration without genesis-freezing pressure.
- The registration ceremony doubles as the canonical pattern for every subsequent first-party schema (Mneme, Iris, Kleidon products).

The trade-off: a ~30-second window between block 0 and the block where `RegisterSchema` lands during which the schema doesn't exist on-chain. For devnet that's invisible to anyone outside the operator team.

## Pre-requisites

- Node booted, `/v1/rollup/info` returning a non-zero `chain_hash`, blocks producing every ~12s.
- Operator host has the workspace built (`cargo build --release -p ligate-bootstrap-cli` once, ~20s warm).
- Operator key file on disk (e.g. `/etc/ligate/keys/themisra-bootstrap.key`), funded with at least 1 LGT (covers `attestor_set_fee` 0.05 LGT + `schema_registration_fee` 0.1 LGT plus generous tx-fee headroom).
- `devnet-1/canonical-schemas.toml` exists (copy of the `.example` with real attestor pubkeys filled in).

## Step 1 — Build + sanity-check

```sh
# Build the binary (one-time, ~20s warm).
cargo build --release -p ligate-bootstrap-cli

# Sanity-check the canonical payload_shape_hash for proof-of-prompt/v1.
# Output is deterministic; this matches what the binary will register.
./target/release/ligate-bootstrap compute-shape-hash \
    docs/protocol/schemas/themisra.proof-of-prompt-v1.json
# → 9ac4625714b18111f53237f7906ad3bba1e8d2a584db5afd8cc3f823cbed1bdf
```

If the hash differs from what's documented in v0.1.0-devnet's release notes, **stop**. Either the schema doc changed (re-pin in release notes) or the file's canonical form drifted (run `prettier --write` on it; re-check; the binary catches BOM, CRLF, missing trailing newline).

## Step 2 — Fill in `canonical-schemas.toml`

```sh
cp devnet-1/canonical-schemas.toml.example devnet-1/canonical-schemas.toml
$EDITOR devnet-1/canonical-schemas.toml
```

Substitute the placeholder hex pubkeys (`0000…`, `1111…`, `2222…`) with the real attestor pubkeys from your federated-quorum partners. Order matters — the chain sorts by raw bytes for `attestor_set_id` derivation, but the order in this file is what the human sees in the registered payload.

Don't commit the filled-in file. `devnet-1/.gitignore` already excludes it.

## Step 3 — Run the ceremony

```sh
./target/release/ligate-bootstrap register-canonical-schemas \
    --config devnet-1/canonical-schemas.toml \
    --signer-key /etc/ligate/keys/themisra-bootstrap.key \
    --rpc http://localhost:12346 \
    --chain-id 4242
```

`--chain-id` is the numeric `CHAIN_ID` defined in `constants.toml` (NOT the human-readable `ligate-devnet-1` string). `--chain-hash` is fetched from `GET /v1/rollup/info` automatically; pass `--chain-hash <64-char-hex>` to override (the flag expects hex, optionally with a `0x` prefix; not the bech32m `lsch1…` form).

Expected output (success path):

```
connected to ligate-devnet-1 (ligate-node 0.1.0-devnet)

Canonical schema registration: 1 schema(s)

  themisra.proof-of-prompt v1
    attestor_set_id    : las1...
      → submitted (new)
    schema_id          : lsc1...
      → submitted (new)
    payload_shape_hash : 9ac4625714b18111f53237f7906ad3bba1e8d2a584db5afd8cc3f823cbed1bdf
```

Each `submitted (new)` line means the corresponding tx landed and was indexed by the chain (HTTP-poll on `/v1/ledger/txs/{hash}`, 30-second timeout). The total wall-clock time for one schema is ~24-36s (two slot intervals).

## Step 4 — Verify

```sh
# Pull the registered schema. Should return a 200 with the schema_id
# and payload_shape_hash you expect.
curl -s http://localhost:12346/v1/modules/attestation/schemas/lsc1... | jq .

# Pull the attestor set.
curl -s http://localhost:12346/v1/modules/attestation/attestor-sets/las1... | jq .
```

Both should return `200 OK` with bodies matching the IDs printed in step 3.

## Step 5 — Publish the canonical IDs

The `schema_id` (bech32m `lsc1...`) and `attestor_set_id` (`las1...`) are the canonical handles partners use to query the chain. Publish them in:

- This repo's `CHANGELOG.md` under the `v0.1.0-devnet` entry.
- `docs.ligate.io/devnet/schemas` (marketing site, post-launch).
- `ligate-js` README + cli docs.

Once published, downstream code can compute the same `schema_id` offline via `Schema::derive_id(<owner_addr>, "themisra.proof-of-prompt", 1)` (Rust) or the equivalent `@ligate/sdk` helper (TypeScript), so consumers don't have to round-trip the chain to know what id to query.

## Re-running

The script is idempotent. If you SIGINT halfway through, or the chain rejects a tx for any reason (insufficient fee, stale nonce), just run the same command again. Already-registered ids are skipped:

```
themisra.proof-of-prompt v1
    attestor_set_id    : las1...
      → already registered (skipped)
    schema_id          : lsc1...
      → submitted (new)
```

## Adding a future schema

1. Author the JSON Schema doc at `docs/protocol/schemas/<name>-v<version>.json`. Run prettier on it.
2. Verify it passes the canonical-form check: `./target/release/ligate-bootstrap compute-shape-hash <path>`.
3. Add a new `[[schemas]]` block to `devnet-1/canonical-schemas.toml.example`.
4. Re-run `register-canonical-schemas`. The existing entries are skipped; only the new one lands.

## Troubleshooting

- **`HTTP 503` from sequencer on submit**: the node is in `--mode follower` (chain #248). Re-target `--rpc` at the canonical sequencer host.
- **`timed out waiting for tx ... to be included`**: check the node is producing blocks (`grep "Sealed slot" /var/log/ligate-node.log`). The tx may still land later; query `/v1/ledger/txs/{hash}` manually to confirm.
- **`CannotReserveGas("Insufficient balance")`**: the signer key's address has fewer than ~0.15 LGT (the ceremony fee total: 0.05 attestor-set + 0.1 schema-reg). Faucet-drip more, retry.
- **`payload_shape_hash` mismatch with documented value**: the local schema file drifted from the canonical form. Restore it from `git show main:docs/protocol/schemas/...` or apply prettier; the binary's canonical-form check (BOM / CRLF / trailing newline) catches the common drift.
- **Schema name conflict**: schema_id is namespaced by owner address, so `(your_addr, "themisra.proof-of-prompt", 1)` is a different id than `(ligate_labs_addr, "themisra.proof-of-prompt", 1)`. There's no global namespace lock; the canonical Themisra schema is "the one owned by Ligate Labs's published address."

## Related

- [`docs/protocol/schemas/`](../protocol/schemas/) — checked-in JSON Schema docs
- [`crates/bootstrap-cli/`](../../crates/bootstrap-cli/) — the binary that runs the ceremony
- [`crates/modules/attestation/`](../../crates/modules/attestation/) — on-chain attestation module
- [`docs/development/public-devnet-deploy.md`](public-devnet-deploy.md) — the boot runbook this ceremony comes after
