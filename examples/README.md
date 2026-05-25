# `examples/`: docker-compose stack

One-command bring-up of `ligate-node` + a co-located Celestia light node, both as Docker containers, wired to the committed `devnet-1/` config.

## Quickstart (follower mode)

```bash
git clone https://github.com/ligate-io/ligate-chain
cd ligate-chain/examples
cp .env.example .env
docker-compose up -d
```

Wait ~10 minutes for Celestia to header-sync, then:

```bash
curl http://localhost:12346/v1/rollup/info
# {"chain_id":"ligate-devnet-1","chain_hash":"...","version":"0.3.0"}

curl http://localhost:12346/health
# {"status":"ok"}

curl http://localhost:12346/ready
# 200 once /v1/rollup/sync-status reports caught up
```

## Sequencer mode

Add your signer key to `.env`:

```
SOV_CELESTIA_SIGNER_KEY=<hex private key holding Mocha TIA>
```

Get free Mocha TIA from the [Celestia faucet](https://docs.celestia.org/nodes/mocha-testnet#mocha-testnet-faucet).

Then `docker-compose up -d` as before. The chain will start producing blocks and submitting blobs to Celestia.

## Verify against the canonical chain

A follower can cross-check `chain_hash` against the canonical Ligate Labs node to confirm both run the same runtime composition:

```bash
diff <(curl -s https://rpc.ligate.io/v1/rollup/info | jq -r .chain_hash) \
     <(curl -s http://localhost:12346/v1/rollup/info | jq -r .chain_hash)
# Empty diff => same runtime => safe interop.
```

If the hashes differ, your follower pulled a different binary version OR a different genesis bundle than the canonical chain.

## What's in the stack

Two containers, two volumes, one bridge network:

```
┌─ docker network: ligate ─────────────────────────────────┐
│                                                          │
│  celestia-light (ghcr.io/celestiaorg/celestia-node)      │
│    ├─ command: light start --p2p.network=mocha           │
│    ├─ volume: celestia-data → /home/celestia             │
│    └─ JSON-RPC at celestia-light:26658 (internal only)   │
│                                                          │
│  ligate-node (ghcr.io/ligate-io/ligate-chain)            │
│    ├─ depends_on: celestia-light                         │
│    ├─ env: SOV_CELESTIA_RPC_URL=ws://celestia-light:26658│
│    ├─ volume: ../devnet-1 → /opt/ligate/devnet-1 (ro)    │
│    ├─ volume: ligate-data → /var/lib/ligate              │
│    ├─ chain REST exposed at 127.0.0.1:12346              │
│    └─ Prometheus exposed at 127.0.0.1:9100               │
└──────────────────────────────────────────────────────────┘
```

Both ports bind to `127.0.0.1` only. To expose externally, run a reverse proxy (Caddy, Cloudflare Tunnel, nginx) on the host. See [`../docs/development/public-devnet-deploy.md`](../docs/development/public-devnet-deploy.md) for the canonical Caddy + Let's Encrypt setup.

## When to use this vs the systemd path

| | Compose (this dir) | Systemd (deploy runbook) |
|---|---|---|
| Single host, casual operator | ✓ | ✓ |
| Production sequencer (Ligate Labs) | OK | preferred |
| Multi-host k8s | ⨯ (needs Helm) | ⨯ (needs k8s manifests) |
| Cleanest cgroup / journald integration | OK | preferred |
| Lowest cognitive overhead for a follower | preferred | OK |

Pick whichever your ops culture prefers. The chain inside is identical.

## Substituting your own genesis

The committed `../devnet-1/genesis/` uses placeholder addresses. Real public deploys substitute operator-controlled keys via [`ligate-genesis-tool`](../crates/genesis-tool/) (#191):

```bash
# From the repo root, with your operator keys in keys.toml:
cargo run -p ligate-genesis-tool -- generate \
    --template devnet-1/genesis \
    --substitutions keys.toml \
    --output /tmp/my-devnet-1/genesis

# Then edit examples/docker-compose.yaml to mount /tmp/my-devnet-1
# instead of ../devnet-1, and restart:
docker-compose down
docker-compose up -d
```

## Cleanup

```bash
docker-compose down       # stop containers, keep volumes
docker-compose down -v    # stop + delete volumes (full reset)
```

## Tracking issues

- Compose file: [#221](https://github.com/ligate-io/ligate-chain/issues/221)
- Underlying images: [#194](https://github.com/ligate-io/ligate-chain/issues/194) (releases), [#195](https://github.com/ligate-io/ligate-chain/issues/195) (Docker)
- Genesis tool: [#191](https://github.com/ligate-io/ligate-chain/issues/191)
