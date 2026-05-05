# Ligate `ligate-devnet-1` config

Public-devnet artifacts for the canonical first rung of the chain-id ladder. Sibling of `devnet/` (localnet); operators target this directory when bringing up a node that joins the public Ligate Chain devnet on Celestia Mocha.

## Layout

```
devnet-1/
├── celestia.toml      # rollup config (chain identity, DA, storage, sequencer)
├── genesis/           # one JSON per runtime module
│   ├── accounts.json
│   ├── attestation.json
│   ├── attester_incentives.json
│   ├── bank.json
│   ├── chain_state.json
│   ├── operator_incentives.json
│   ├── prover_incentives.json
│   └── sequencer_registry.json
└── README.md
```

No `rollup.toml`. Devnet-1 is Celestia-only by design; MockDA only exists in the localnet config (`devnet/`) for local smoke testing. If you want a MockDA flavour, use `devnet/`, not `devnet-1/`.

## Boot

From the repo root:

```sh
cargo run --bin ligate-node -- \
    --da-layer celestia \
    --rollup-config-path devnet-1/celestia.toml \
    --genesis-config-dir devnet-1/genesis
```

Production deploys override secrets via env vars; never commit them:

```sh
SOV_CELESTIA_RPC_URL=ws://127.0.0.1:26658 \
SOV_CELESTIA_GRPC_URL=http://127.0.0.1:9090 \
SOV_CELESTIA_SIGNER_KEY=$(pass celestia/devnet-1-signer) \
cargo run --bin ligate-node -- \
    --da-layer celestia \
    --rollup-config-path devnet-1/celestia.toml \
    --genesis-config-dir devnet-1/genesis
```

The full deploy runbook lives at [`docs/development/public-devnet-deploy.md`](../docs/development/public-devnet-deploy.md) and covers VM provisioning, the co-located Celestia light node, Caddy + Let's Encrypt, monitoring, and failure-mode triage.

## Differences from `devnet/`

| Field | `devnet/` (localnet) | `devnet-1/` (public devnet) |
|---|---|---|
| `chain_id` | `ligate-localnet` | `ligate-devnet-1` |
| DA layer | MockDA + Celestia (two flavours) | Celestia only |
| Default `rpc_url` | `ws://127.0.0.1:26658` | same; pattern assumes co-located light node |
| `storage.path` | `devnet/data-celestia` | `devnet-1/data-celestia` (lets a single host run both) |
| Bind host | `127.0.0.1` | `127.0.0.1` (public TLS via Caddy on same VM) |
| Genesis allocations | 3 dev wallets, anvil-style keys | placeholder (see "Placeholder addresses" below) |

## Placeholder addresses (read this before you deploy)

The genesis JSONs in this directory carry the **same placeholder addresses as `devnet/`**, which are deterministic test fixtures derived from public string seeds. They exist so the chain boots cleanly for CI parse tests and `cargo run` smoke. **Do not deploy a public node with these addresses unchanged.** The private keys behind those addresses are derivable from a public seed; anyone could impersonate the bootstrap actor, sequencer, treasury, attester, or prover.

For the real public-devnet ceremony:

1. Generate operator-controlled keys offline (`ligate-cli keys generate`, [#112](https://github.com/ligate-io/ligate-chain/issues/112)).
2. Run the genesis-generation tool ([#191](https://github.com/ligate-io/ligate-chain/issues/191)) with those keys to produce a real `genesis/` bundle.
3. Replace the contents of `devnet-1/genesis/` with the real bundle (private keys stay offline; only the addresses go in the JSONs).
4. Commit the real bundle as a one-shot ceremony PR. Public devnet boots from that commit.

Steps 2-3 are tracked in [#191](https://github.com/ligate-io/ligate-chain/issues/191); until that tool ships, the manual path is to derive addresses by hand and edit the JSONs (slow, error-prone).

## Mocha resets and `devnet-2/`

Mocha (Celestia's testnet) resets occasionally. When it does, the DA-layer references in our chain state become invalid; the cleanest recovery is to spin up a fresh chain id (`ligate-devnet-2`) on the new Mocha epoch with a fresh genesis bundle. The directory naming pattern this template establishes (`devnet-1/` → `devnet-2/` → ... → `ligate-1/`) supports that without touching the existing artifacts.

## Faucet

[#95](https://github.com/ligate-io/ligate-chain/issues/95) tracks the faucet service. The faucet hot key is intentionally separate from the bootstrap (treasury) key so a faucet drain does not touch the treasury balance. When the genesis ceremony runs, the faucet account is provisioned in `bank.json` with its own balance and a separate `lig1...` address; the faucet service signs from that key.

The placeholder bundle here uses the same bootstrap address everywhere as `devnet/` does, so there is no faucet account in the committed JSONs yet. Real ceremony adds it.

## See also

- [`docs/development/public-devnet-deploy.md`](../docs/development/public-devnet-deploy.md): operator runbook (sequencer + follower flows).
- [`docs/protocol/attestation-v0.md`](../docs/protocol/attestation-v0.md): the spec, including the chain-id ladder section.
- [`devnet/README.md`](../devnet/README.md): localnet variant; useful for local smoke testing without Celestia.
- [`crates/rollup/src/chain_config.rs`](../crates/rollup/src/chain_config.rs): the loader that splits `[chain]` from the SDK's `RollupConfig` and validates the ladder.
- [#181](https://github.com/ligate-io/ligate-chain/issues/181): the ladder validator.
- [#188](https://github.com/ligate-io/ligate-chain/issues/188): this directory.
- [#191](https://github.com/ligate-io/ligate-chain/issues/191): genesis generation tool.
