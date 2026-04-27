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

A single chain-native account does everything at genesis to keep
the boot story narrow:

| Role | Address | Notes |
|---|---|---|
| Treasury / bank admin | `lig1h72nh5c7jfjkcygku4thsh2t53dyh33kkpktpy84w06qwr4agvt` | Receives attestation fees. |
| Initial sequencer (rollup) | same | Locks 5 000 LGT collateral |
| Initial sequencer (DA) | `0000…0000` (32 bytes hex) | DA-layer address (Celestia / MockDa). Unrelated to Ligate accounts; lives in a different cryptographic namespace. |
| Initial attester | same | Bonds 1 000 LGT |
| Initial prover | same | Bonds 1 000 LGT |
| Operator reward addr | same | |
| Prover reward addr | same | |

The two other accounts seeded with `$LGT`:

- `lig1d0vqhkvnlaga6xe8rfed900qxnzhqnmhweqe3rxqdexp56lvqv7` — dev wallet 1
- `lig1njjerykggkxvwa55x4lupkl67qnxsx9rwqxpuq4s3cmh7vmn7kx` — dev wallet 2

These let you exercise transfers without minting more.

**Address derivation.** All three are deterministic:
`SHA256("ligate-devnet-{bootstrap,dev-1,dev-2}")[..28]`, encoded
Bech32m with the `lig` HRP from `constants.toml`. The
`crates/stf/tests/devnet_addresses.rs` test re-derives them and
fails if the genesis files drift.

**Why not `0x…` Ethereum addresses.** The chain's tx authentication
path uses ed25519 (via `RollupAuthenticator`); MetaMask /
secp256k1 wallets cannot natively sign for it today. Funding `0x…`
addresses creates accounts that can receive but never spend — they
were misleading dev fixtures and we removed them. EVM-shaped
authentication is tracked separately in
[#72](https://github.com/ligate-io/ligate-chain/issues/72) and
lands before any retail wallet UX.

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

## ZK proving (Celestia flavour only)

The Celestia blueprint ships with a real Risc0 inner zkVM wired
through to a guest binary in
[`crates/rollup/provers/risc0/`](../crates/rollup/provers/risc0/).
The Mock-DA flavour stays on MockZkvm.

To actually generate proofs locally:

1. Install the risczero toolchain:
   ```sh
   curl -L https://risczero.com/install | bash
   rzup install
   ```
2. Build the rollup with the guest enabled (drops the
   `SKIP_GUEST_BUILD=1` default CI uses):
   ```sh
   cargo build --bin ligate-node
   ```
   First build takes ~10 minutes; subsequent rebuilds are cached.
3. Update `genesis/chain_state.json`'s `inner_code_commitment`
   to the real image ID. Read it from
   `target/debug/build/ligate-prover-risc0-*/out/methods.rs`
   after the build, then commit the change. The placeholder
   `[0; 8]` works only with `SOV_PROVER_MODE` unset (proving
   disabled).
4. Set `SOV_PROVER_MODE=Prove` and run as usual.

CI keeps `SKIP_GUEST_BUILD=1` and `RISC0_SKIP_BUILD_KERNELS=1`
set; everyday iteration doesn't pay the toolchain tax. A
dedicated proving workflow runs the full guest build on a
schedule (follow-up).

## What's _not_ here

- **Real attestor sets / schemas.** `initial_attestor_sets` and
  `initial_schemas` in `attestation.json` are intentionally empty.
  Schema registration via tx after boot is the same code path —
  better to exercise it than to bake test fixtures into genesis.
- **Real `inner_code_commitment` in `chain_state.json`.** Still
  `[0; 8]` placeholder. Updates when somebody runs the full
  guest build and commits the result; tracked by the prove-on-CI
  follow-up workflow.
- **Mainnet namespaces.** The `lig-dvn-*` namespaces are devnet-
  scoped; mainnet picks fresh ones to avoid devnet replay risk.
