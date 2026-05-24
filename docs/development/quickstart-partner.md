# Partner Quickstart — Zero to First Attestation on `ligate-devnet-1`

**Status:** Pre-devnet. The flow below is what `ligate-devnet-1` will support on Day 1 (target Q2 2026). Sections marked with **⚠ Preview** are wired in code but the public infrastructure they depend on (faucet, hosted RPC, canonical schemas, explorer) is still spinning up.

This page takes a partner from "I just heard about Ligate" to "my first attestation landed on the public chain" in one continuous flow. No tribal knowledge, no jumping between three doc sites.

If you're building the protocol (chain core, modules, SDK), [`docs/protocol/attestation-v0.md`](../protocol/attestation-v0.md) is the right entry point instead.

---

## 0. What you'll need

| Item | Source |
|---|---|
| `ligate-cli` binary | Pre-built tarball from [`ligate-io/ligate-cli` releases](https://github.com/ligate-io/ligate-cli/releases), or `cargo install --git` (fallback). [Install docs](https://github.com/ligate-io/ligate-cli#install). |
| A ligate address with `$AVOW` | Generate locally + claim from the devnet faucet. Steps 1–3. |
| Optional: an attestor set you trust | The canonical Themisra set ships with devnet-1. Step 4 documents how to discover it. |

You do NOT need:

- A node binary. Partners talk to the hosted public RPC, not their own node.
- A keystore service. `ligate-cli` writes keys to the OS-default data dir (mode 0600) by default.

---

## 1. Install `ligate-cli`

```bash
# Pick the platform tarball from the latest release. PLATFORM is one
# of: linux-amd64, linux-arm64, darwin-arm64, darwin-amd64.
curl -L -o ligate.tar.gz \
  https://github.com/ligate-io/ligate-cli/releases/latest/download/ligate-PLATFORM.tar.gz
tar -xzf ligate.tar.gz
sudo mv ligate /usr/local/bin/
ligate --help
```

Verify the binary speaks to the public devnet:

```bash
$ ligate --rpc https://rpc.ligate.io info
chain_id:   ligate-devnet-1
chain_hash: lsch1amq80arndh6zehd4gu3kg6x66vh3l45z924dr6pzeevkxp649heqe5c70v
version:    0.1.3
```

Set `LIGATE_RPC=https://rpc.ligate.io` in your shell so subsequent commands don't need `--rpc`.

---

## 2. Generate a keypair

```bash
$ ligate keys generate --name first
generated keypair:
  role:    first
  address: lig1u8z2rxh6ymjwkqsasme64f5kfphtfm2kf4kkn0clusfpr34amezsp5j7yp
  key:     ~/Library/Application Support/io.ligate.cli/keys/first.key (mode 0600)
```

The private key never leaves your machine. The address is what partners trade and the chain references. Address derivation rule: `pubkey[..28]`, bech32m-encoded with the `lig` HRP.

If you want to inspect the address later:

```bash
$ ligate keys show first
lig1u8z2rxh6ymjwkqsasme64f5kfphtfm2kf4kkn0clusfpr34amezsp5j7yp
```

---

## 3. Get `$AVOW` from the faucet

⚠ **Preview**: `faucet.ligate.io` infrastructure is on the pre-devnet checklist. Until then, the flow uses the same endpoint from a localnet node.

```bash
$ ligate faucet $(ligate keys show first)
dripped 1 $AVOW to lig1u8z2rxh6ymjwkqsasme64f5kfphtfm2kf4kkn0clusfpr34amezsp5j7yp
tx_hash: ltx1qqu226ttcgnq8rv5x0klvxalv9pkk2hwkj8s8w834a4p7tgznvts0sekwe
```

Verify the balance arrived (the faucet waits for chain inclusion before returning, so this should be non-zero immediately):

```bash
$ ligate balance $(ligate keys show first) \
    --token-id token_1nyl0e0yweragfsatygt24zmd8jrr2vqtvdfptzjhxkguz2xxx3vs0y07u7
lig1u8z2rxh6ymjwkqsasme64f5kfphtfm2kf4kkn0clusfpr34amezsp5j7yp: 1.000000000 $AVOW (1000000000 nano)
```

Rate limit: one drip per address per 24 hours on devnet-1. If you need more for testing, ask in `#dev` (Discord pending) or run a localnet node.

---

## 4. Pick a canonical schema

⚠ **Preview**: canonical schemas land at genesis on devnet-1. List them via the indexer:

```bash
$ curl -s https://api.ligate.io/v1/schemas | jq -r '.data[] | "\(.id) \(.name) v\(.version)"'
lsc1abc... themisra.proof-of-prompt 1
lsc1def... themisra.content-provenance 1
```

For a partner just exploring, **`themisra.proof-of-prompt/v1`** is the canonical demo schema: it attests "this prompt was sent to this model and produced this output." Documented in [LIP-1 (RFC 0001 in the marketing repo)](https://docs.ligate.io/lips/lip-1).

Inspect one:

```bash
$ curl -s https://api.ligate.io/v1/schemas/lsc1abc... | jq
{
  "id": "lsc1abc...",
  "name": "themisra.proof-of-prompt",
  "version": 1,
  "owner": "lig1...",
  "attestor_set_id": "las1xyz...",
  "fee_routing_bps": 0,
  "fee_routing_addr": null,
  "payload_shape_hash": "deadbeef...",
  "registered_at": { "block_height": 12345, "tx_hash": "ltx1...", "timestamp": "2026-05-12T01:23:45.678Z" },
  "attestation_count": 1234
}
```

The `payload_shape_hash` is what every attestation must commit to. You hash your evidence document the same way (SHA-256 of canonical bytes) and pass it as `--payload-shape-hash` in step 5.

---

## 5. Submit your first attestation

⚠ **Preview**: `ligate-cli attest` subcommand is on the pre-devnet feature list (chain repo issue #112's v0 surface). Until it ships, the chain repo's `bootstrap-cli` is the path; see [`devnet/README.md`](../../devnet/README.md).

For the v0 partner flow with `ligate-cli`:

```bash
$ EVIDENCE_HASH=$(echo -n '{"prompt":"hello","output":"hi"}' | sha256sum | awk '{print $1}')
$ ligate attest \
    --schema lsc1abc... \
    --payload-hash "lph1$(echo $EVIDENCE_HASH | bech32 encode lph)" \
    --signer first \
    --chain-hash "$(ligate info --json | jq -r .chain_hash)" \
    --chain-id 4242
submitted attestation:
  schema_id:    lsc1abc...
  payload_hash: lph1...
  tx_hash:      ltx1xyz...
  outcome:      committed
```

The `--chain-hash` and `--chain-id 4242` bind the signature to the chain so it can't be replayed against testnet or another devnet revision. Pull `chain-hash` from `ligate info --json`; the numeric `chain-id` is fixed (`4242` for ligate-devnet-1; the Cosmos-style string id `ligate-devnet-1` is separate).

---

## 6. Verify it landed

Two ways:

```bash
# Via ligate-cli (uses the chain RPC directly)
$ ligate get-tx ltx1xyz...
  block_height: 12999
  block_hash:   lblk1...
  outcome:      committed
  kind:         submit_attestation
  details: { schema_id: ..., payload_hash: ..., signature_count: 3 }

# Via the indexer (faster for explorer-style views)
$ curl -s https://api.ligate.io/v1/txs/ltx1xyz... | jq
```

Or in the browser, open `https://explorer.ligate.io/tx/ltx1xyz...`.

---

## 7. (Optional) Register your own schema

If the canonical schemas don't fit your use case, register your own. Trade-off: you bind to your own attestor set (which you supply pubkeys for), and you pay the registration fee.

```bash
# 1. Pick or register an attestor set. For a solo proof-of-concept,
#    bind to a 1-of-1 set with your own pubkey.
$ MY_PUBKEY=$(ligate keys show first --pubkey)
$ ligate register-attestor-set \
    --member "$MY_PUBKEY" \
    --threshold 1 \
    --signer first

# 2. Hash your schema doc (canonical JSON Schema, sorted keys, no trailing
#    whitespace). Same SHA-256 the chain uses.
$ SHAPE_HASH=$(jq -S -c . my-schema.json | sha256sum | awk '{print $1}')

# 3. Register the schema.
$ ligate register-schema \
    --name "mycompany.my-schema" \
    --version 1 \
    --attestor-set las1abc... \
    --payload-shape-hash "0x${SHAPE_HASH}" \
    --signer first
```

Schema id derivation: `SHA-256(owner_addr || name || version_le)`. Deterministic — you can compute it offline before submission.

---

## 8. What's next

- [Chain protocol spec](../protocol/attestation-v0.md) — types, state, invariants, fees.
- [`ligate-js`](https://github.com/ligate-io/ligate-js) — TypeScript SDK if you'd rather build in Node/browser than shell out to `ligate-cli`.
- [`ligate-api`](https://github.com/ligate-io/ligate-api) — REST surface the explorer uses; same endpoints partners can query directly.
- [Mneme](https://mneme.xyz) (status: pre-devnet) — first-party browser wallet that signs attestations from a single click. Replaces the `ligate-cli` step for end users.

---

## Help

- File chain issues at [`ligate-io/ligate-chain`](https://github.com/ligate-io/ligate-chain/issues).
- File CLI / SDK issues at [`ligate-io/ligate-cli`](https://github.com/ligate-io/ligate-cli/issues) or [`ligate-io/ligate-js`](https://github.com/ligate-io/ligate-js/issues).
- Reach Ligate Labs at `hello@ligate.io`.

⚠ Discord pending — link will land here when it ships.
