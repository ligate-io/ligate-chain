# Ligate `ligate-devnet-2` config

Public-devnet artifacts for the canonical first rung of the chain-id ladder. Sibling of `devnet/` (localnet); operators target this directory when bringing up a node that joins the public Ligate Chain devnet on Celestia Mocha.

## Layout

```
devnet/
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

No `rollup.toml`. Devnet-2 is Celestia-only by design; MockDA only exists in the localnet config (`devnet/`) for local smoke testing. If you want a MockDA flavour, use `devnet/`, not `devnet/`.

## Boot

From the repo root:

```sh
cargo run --bin ligate-node -- \
    --da-layer celestia \
    --rollup-config-path devnet/celestia.toml \
    --genesis-config-dir devnet/genesis
```

Production deploys override secrets via env vars; never commit them:

```sh
SOV_CELESTIA_RPC_URL=ws://127.0.0.1:26658 \
SOV_CELESTIA_GRPC_URL=http://127.0.0.1:9090 \
SOV_CELESTIA_SIGNER_KEY=$(pass celestia/devnet-signer) \
cargo run --bin ligate-node -- \
    --da-layer celestia \
    --rollup-config-path devnet/celestia.toml \
    --genesis-config-dir devnet/genesis
```

The full deploy runbook lives at [`docs/development/public-devnet-deploy.md`](../docs/development/public-devnet-deploy.md) and covers VM provisioning, the co-located Celestia light node, Caddy + Let's Encrypt, monitoring, and failure-mode triage.

## Differences from `devnet/`

| Field | `devnet/` (localnet) | `devnet/` (public devnet) |
|---|---|---|
| `chain_id` | `ligate-localnet` | `ligate-devnet-2` |
| DA layer | MockDA + Celestia (two flavours) | Celestia only |
| Default `rpc_url` | `ws://127.0.0.1:26658` | same; pattern assumes co-located light node |
| `storage.path` | `devnet/data-celestia` | `devnet/data-celestia` (lets a single host run both) |
| Bind host | `127.0.0.1` | `127.0.0.1` (public TLS via Caddy on same VM) |
| Genesis allocations | 3 dev wallets, anvil-style keys | placeholder (see "Placeholder addresses" below) |

## Placeholder addresses (read this before you deploy)

The genesis JSONs in this directory carry the **same placeholder addresses as `devnet/`**, which are deterministic test fixtures derived from public string seeds. They exist so the chain boots cleanly for CI parse tests and `cargo run` smoke. **Do not deploy a public node with these addresses unchanged.** The private keys behind those addresses are derivable from a public seed; anyone could impersonate the bootstrap actor, sequencer, treasury, attester, or prover.

For the real public-devnet ceremony (tracked in [#231](https://github.com/ligate-io/ligate-chain/issues/231)):

### 1. Generate operator-controlled keys offline

```sh
cargo run -p ligate-genesis-tool -- keys generate \
    --roles operator,demo1,demo2 \
    --output ~/.ligate-keys/devnet
```

Each role yields one Ed25519 keypair: `<role>.key` (hex-encoded, chmod 600, **never commit**) and `<role>.address` (the `lig1...` bech32m string ready to paste into `keys.toml`). Address derivation matches the SDK's standard `Address = SHA-256(pubkey)[..28]`.

Pick role labels that reflect your security model:

- **Single key** (simplest): `--roles operator` covers sequencer / treasury / reward / attester / prover all from one address.
- **Split for security separation**: `--roles sequencer,treasury,operator,demo1,demo2`. Treasury can stay cold (offline), sequencer is necessarily hot (signs continuously).

This subcommand is a stand-in for [`ligate-cli keys generate`](https://github.com/ligate-io/ligate-chain/issues/112); when `ligate-cli` ships in its own repo, the same command moves there.

### 2. Fill in `keys.toml` from the template

```sh
cp devnet/keys.toml.example devnet/keys.toml
$EDITOR devnet/keys.toml
```

`keys.toml` is gitignored. The template enumerates every placeholder address in the current `genesis/` bundle and walks through which roles each one fills.

### 3. Substitute genesis

```sh
cargo run -p ligate-genesis-tool -- generate \
    --template devnet/genesis \
    --substitutions devnet/keys.toml \
    --output /tmp/ligate-devnet-2-real/genesis

cargo run -p ligate-genesis-tool -- verify \
    --dir /tmp/ligate-devnet-2-real/genesis
```

`verify` runs all the cross-module invariants from [#175](https://github.com/ligate-io/ligate-chain/issues/175) (no orphan references, threshold ≤ members, fee routing addresses exist, etc.) and catches typos before the chain refuses to boot.

### 4. Commit the substituted bundle as a ceremony PR

```sh
rm -rf devnet/genesis
cp -r /tmp/ligate-devnet-2-real/genesis devnet/genesis
git add devnet/genesis
git commit -m "ceremony: substitute devnet genesis with operator-controlled keys (closes #231)"
```

Public devnet boots from that commit. Private keys never leave your machine.

### 5. Source Mocha TIA for the Celestia hot key

The Celestia hot key (`SOV_CELESTIA_SIGNER_KEY` env var) is **separate** from the genesis substitution above. It signs blob inclusion txs and pays Mocha TIA fees. Generate a Celestia keypair, derive its address, then request ~10-50 TIA from the [Celestia Mocha faucet](https://docs.celestia.org/nodes/mocha-testnet#mocha-testnet-faucet) Discord bot.

Set the env var in `/etc/ligate/env` on the GCP VM (`chmod 600`, root-readable only) per the deploy runbook.

### 6. Register canonical schemas (post-boot)

`devnet/genesis/attestation.json` ships with `initial_attestor_sets: []` and `initial_schemas: []` deliberately. After the node boots and `/v1/rollup/info` returns a non-zero `chain_hash`, register the canonical Themisra schema (`themisra.proof-of-prompt/v1`) by running the `ligate-bootstrap` ceremony:

```sh
cp devnet/canonical-schemas.toml.example devnet/canonical-schemas.toml
$EDITOR devnet/canonical-schemas.toml  # fill in real attestor pubkeys
cargo run --release -p ligate-bootstrap-cli -- register-canonical-schemas \
    --config devnet/canonical-schemas.toml \
    --signer-key ~/.ligate-keys/devnet/operator.key \
    --rpc https://rpc.ligate.io \
    --chain-id 4242
```

Idempotent: re-runs skip already-landed registrations. Full runbook (verification, troubleshooting, adding future schemas) at [`docs/development/canonical-schema-registration.md`](../docs/development/canonical-schema-registration.md).

This step is **post-genesis on purpose**: it keeps the chain's "attestation primitive is permissionless from day one" promise honest (Ligate Labs registers the same way any third-party schema would) and avoids baking a placeholder `payload_shape_hash: [0u8; 32]` into permanent chain state. The registered schema is bit-for-bit identical to a genesis-seeded one once Tx 2 lands ~24-36s after the script starts.

## Mocha resets and `devnet-2/`

Mocha (Celestia's testnet) resets occasionally. When it does, the DA-layer references in our chain state become invalid; the cleanest recovery is to spin up a fresh chain id (`ligate-devnet-2`) on the new Mocha epoch with a fresh genesis bundle. The directory naming pattern this template establishes (`devnet/` → `devnet-2/` → ... → `ligate-1/`) supports that without touching the existing artifacts.

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
