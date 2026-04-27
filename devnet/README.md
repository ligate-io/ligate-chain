# Ligate Chain devnet config

Single-node MockDa + MockZkvm devnet, ready to boot:

```sh
cargo run --bin ligate-node \
  --rollup-config-path devnet/rollup.toml \
  --genesis-config-dir   devnet/genesis
```

(Defaults match the layout, so a bare `cargo run --bin ligate-node`
also works from the repo root.)

## What's here

- `rollup.toml` — node-level config: storage paths, DA, sequencer,
  proof manager. Annotated inline.
- `genesis/*.json` — one file per runtime module the chain
  initialises at genesis. See [`ligate_stf::genesis_config`]
  (`crates/stf/src/genesis_config.rs`) for the loader contract and
  the cross-module invariants that get checked before the chain
  starts.

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
(`0x70997970…`) and #2 (`0x3C44Cd…`) — useful for end-to-end
transfer testing without minting more.

## $LGT layout

- 9 decimals (Solana-style nano units)
- Genesis supply: 120M LGT
  - Bootstrap: 100M LGT
  - Anvil #1: 10M LGT
  - Anvil #2: 10M LGT
- `lgt_token_id` is the deterministic `config_gas_token_id()` from
  `constants.toml`. The genesis loader cross-checks this against
  `attestation.lgt_token_id` and refuses to boot on mismatch.

## What's _not_ here

- **Real attestor sets / schemas.** `initial_attestor_sets` and
  `initial_schemas` in `attestation.json` are intentionally empty.
  Schema registration via tx after boot is the same code path —
  better to exercise it than to bake test fixtures into genesis.
- **Real chain-id binding.** The runtime ships with `CHAIN_HASH =
  [0u8; 32]` until the build-script slice lands. Anyone with the
  same SDK pin and matching JSONs would produce a hash-compatible
  chain. Fine for a closed devnet, blocks any external sequencer
  story until we wire the real schema-derived hash.
- **Multi-node story.** The MockDa adapter is single-process. Phase
  A.3 swaps it for Celestia and a real DA + sequencer split.
