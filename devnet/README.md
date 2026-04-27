# Ligate Chain devnet config

Two pre-built rollup configs share one set of genesis JSONs:

| Flavour | DA layer | When to use |
|---|---|---|
| `rollup.toml` | In-process MockDa (SQLite) | Default. Local dev, no external services. |
| `celestia.toml` | Real Celestia (Mocha or local node) | Multi-node experiments, DA-cost testing. |

## Boot the mock-DA flavour

```sh
cargo run --bin ligate-node
```

Defaults match the layout (`devnet/rollup.toml` + `devnet/genesis/`),
so the bare command works from the repo root.

## Boot the Celestia flavour

```sh
# Mocha public RPC, signer key from your local secret store:
SOV_CELESTIA_RPC_URL=wss://celestia-mocha.public-rpc.com \
SOV_CELESTIA_SIGNER_KEY=$(pass celestia/devnet-signer) \
cargo run --bin ligate-node -- --da-layer celestia
```

Or against a local light node (typical for CI / soak testing):

```sh
SOV_CELESTIA_RPC_URL=ws://127.0.0.1:26658 \
SOV_CELESTIA_SIGNER_KEY=... \
cargo run --bin ligate-node -- --da-layer celestia
```

The checked-in `celestia.toml` has placeholder values for
`rpc_url`, `grpc_url`, and `signer_private_key` — the binary will
read overrides from `SOV_CELESTIA_RPC_URL`,
`SOV_CELESTIA_GRPC_URL`, and `SOV_CELESTIA_SIGNER_KEY` if set, so
secrets never need to land in the repo.

## What's here

- `rollup.toml` — MockDa node config.
- `celestia.toml` — Celestia node config.
- `genesis/*.json` — one file per runtime module the chain
  initialises at genesis. **Shared between both flavours** — only
  the DA layer differs, not the rollup state. See
  [`ligate_stf::genesis_config`](../crates/stf/src/genesis_config.rs)
  for the loader contract and the cross-module invariants that get
  checked before the chain starts.

## Bootstrap actor

A single Anvil dev account does everything at genesis to keep the
boot story narrow:

| Role | Address | Notes |
|---|---|---|
| Treasury / bank admin | `0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266` | Anvil #0. Receives attestation fees. |
| Initial sequencer (rollup) | same | Locks 5 000 LGT collateral |
| Initial sequencer (DA) | `0x0000…0000` | 32-byte mock-DA address |
| Initial attester | same | Bonds 1 000 LGT |
| Initial prover | same | Bonds 1 000 LGT |
| Operator reward addr | same | |
| Prover reward addr | same | |

The two other accounts seeded with `$LGT` are Anvil #1
(`0x70997970…`) and #2 (`0x3C44Cd…`). Useful for end-to-end
transfer testing without minting more.

## $LGT layout

- 9 decimals (Solana-style nano units).
- Genesis supply: 120M LGT.
  - Bootstrap: 100M LGT.
  - Anvil #1: 10M LGT.
  - Anvil #2: 10M LGT.
- `lgt_token_id` is the deterministic `config_gas_token_id()` from
  `constants.toml`. The genesis loader cross-checks this against
  `attestation.lgt_token_id` and refuses to boot on mismatch.

## Celestia namespaces

Both batch and proof namespaces are pinned in `constants.toml`:

- `BATCH_NAMESPACE = "lig-dvn-tb"` — devnet tx batches.
- `PROOF_NAMESPACE = "lig-dvn-tp"` — devnet zk proofs.

A misconfigured node and the canonical chain can't disagree
silently — the namespaces are compiled into the binary via
`config_value!()`.

## What's _not_ here

- **Real attestor sets / schemas.** `initial_attestor_sets` and
  `initial_schemas` in `attestation.json` are intentionally empty.
  Schema registration via tx after boot is the same code path —
  better to exercise it than to bake test fixtures into genesis.
- **Real proving.** Both DA flavours run against MockZkvm. Phase
  A.4 swaps in Risc0 (or SP1); the diff is contained to the
  `Spec` alias + `create_prover_service` body in each blueprint.
- **Mainnet namespaces.** The `lig-dvn-*` namespaces are devnet-
  scoped; mainnet picks fresh ones to avoid devnet replay risk.
