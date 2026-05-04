# Ligate Chain v0 Devnet Operator Runbook

**Status:** the chain runs end-to-end against MockDa or Celestia mocha-testnet
today (Phase A landed in PRs #65–#74). The single-node boot path, genesis
config loading, attestation module call handlers, and Risc0 inner zkVM are
all wired. Outstanding items are the **federated multi-org topology**
(genesis ceremony, attestor key management) and the **public-facing
infrastructure** (hosted RPC, faucet, explorer) — those sections are
marked **Future work** and link to their tracking issues.

For a fast local-only boot (single-node mock-DA, no ceremony), see
[`devnet/README.md`](../../devnet/README.md). This document is the
multi-operator playbook for when public devnet spins up.

This document is **operations**. For the on-chain protocol (types, state,
invariants, fees), read [`docs/protocol/attestation-v0.md`](../protocol/attestation-v0.md)
first.

---

## 1. Overview

Ligate Chain v0 devnet is a **federated attestation testnet**:

- **Chain id**: `ligate-devnet-1`. The trailing number bumps only on a
  state-breaking restart (full reset, new genesis); soft upgrades via
  the upgrade module ([#42](https://github.com/ligate-io/ligate-chain/issues/42))
  keep the same id. Full ladder (`ligate-localnet` / `ligate-devnet-1` /
  `ligate-testnet-1` / `ligate-1`) is locked in
  [the protocol spec](../protocol/attestation-v0.md#chain-id) per
  [#54](https://github.com/ligate-io/ligate-chain/issues/54).
- 3 to 5 attestor organisations, each running one attestor node, each holding
  an independently generated ed25519 keypair. (Ligate Chain accepts only
  ed25519 signatures today — see
  [`docs/protocol/addresses-and-signing.md`](../protocol/addresses-and-signing.md)
  for the full picture, including why MetaMask doesn't work yet and what
  changes when [#72](https://github.com/ligate-io/ligate-chain/issues/72) lands.)
- One sequencer, operated by Ligate Labs.
- Celestia `mocha` testnet for data availability during the devnet phase.
  Mainnet Celestia comes later.
- Public RPC endpoint served from the sequencer.
- No economic slashing. Attestor misbehaviour is handled socially (the set
  is federated and small). On-chain slashing is a v1 concern, tracked in
  the `disputes` module workstream.

The first-party schema `themisra.proof-of-prompt/v1`, owned by Ligate Labs,
is registered at genesis. Third-party schemas can be registered
permissionlessly after devnet opens (subject to the fee rules in the
protocol spec).

## 2. Topology

```
                        +-----------+
                        | Sequencer | (Ligate Labs)
                        |  + RPC    |
                        +-----+-----+
                              |
                              | blob submit
                              v
                     +-----------------+
                     | Celestia mocha  |
                     +-----------------+

   Attestor org 1       Attestor org 2       ...    Attestor org N
   +------------+       +------------+              +------------+
   | Attestor   |       | Attestor   |              | Attestor   |
   | node       |       | node       |      ...     | node       |
   | (ed25519)  |       | (ed25519)  |              | (ed25519)  |
   +------------+       +------------+              +------------+
```

**What a federated attestor actually does.** An attestor node does two
things:

1. Holds an ed25519 keypair whose public key is listed in some schema's
   `AttestorSet.members` (see protocol spec, section "AttestorSet").
2. Watches the off-chain source-of-truth for that schema. For Themisra's
   `themisra.proof-of-prompt/v1`, that source of truth is the Themisra
   attestor-quorum service: an off-chain HTTP service that receives
   prompt/output material from clients, runs whatever domain checks
   Themisra defines, and produces a canonical `SignedPayload` digest for
   each attestation. The attestor signs that digest with its ed25519
   private key.

An attestor **does not** construct payloads. It signs digests handed to it
by the off-chain quorum service. This keeps the chain layer neutral: the
same attestor node software can serve any schema by pointing it at a
different quorum service.

The submission side (constructing the `CallMessage::SubmitAttestation`
transaction and routing it to the sequencer) is the responsibility of the
schema's client SDK (e.g. `@ligate/themisra`), not the attestor.

## 3. Prerequisites to run a node

The node binary (`ligate-node` in [`crates/rollup`](../../crates/rollup))
boots a single-node rollup against either MockDa (in-process SQLite, the
default) or real Celestia (mocha-testnet or local light/bridge node).
Phase A landed across #65 (`ligate-stf` runtime), #66 (genesis loader +
cross-module validation), #67 (rollup binary on the new SDK), #68
(checked-in devnet config), #69 (`CHAIN_HASH` from runtime schema), #71
(Celestia DA adapter), #74 (Risc0 zkVM + lig1 addresses). The commands
in sections 5 and 6 below run today.

You will need:

- **Rust 1.93.0** via the repo's `rust-toolchain.toml` (pinned). `rustup`
  will pick this up automatically on `cargo build`.
- **RocksDB system dependencies** (required transitively by the Sovereign
  SDK state stores):
  - Ubuntu / Debian: `sudo apt install clang libclang-dev librocksdb-dev`
  - macOS: `brew install rocksdb` (Apple Silicon and Intel)
- **Celestia light node** running against `mocha`, reachable on its local
  RPC. See the [Celestia light-node docs](https://docs.celestia.org/how-to-guides/light-node)
  for install steps. The sequencer and any full nodes need this; attestor
  nodes do not.
- **A persistent state directory** with enough space for RocksDB column
  families. 50 GB is plenty for devnet traffic.
- **An ed25519 keypair** if you are participating as an attestor. Generate
  it on a machine you control (see section 7 on key management).
- Open outbound network access to the sequencer's RPC and to your Celestia
  light node's RPC.

## 4. Genesis ceremony

> **Future work.** The genesis loader runs today (per-module JSON files
> under `devnet/genesis/`, assembled by `crates/stf::genesis_config`).
> What's still future-only is the **multi-org coordination process**
> below — picking attestor pubkeys, agreeing on the threshold, and
> committing a signed-off `genesis.json` set across orgs. That ceremony
> happens once we have ≥ 3 attestor orgs lined up, gated on design
> partner conversations rather than chain code.

The genesis ceremony is a one-time coordinated event to produce a shared
set of per-module genesis JSONs (today: `bank.json`, `accounts.json`,
`sequencer_registry.json`, `operator_incentives.json`,
`attester_incentives.json`, `prover_incentives.json`, `chain_state.json`,
`attestation.json`).

**Who is on the call**

- Ligate Labs (sequencer operator, Themisra schema owner).
- One representative per attestor org. 3 to 5 orgs total for v0.

**What is agreed**

1. The initial `AttestorSet` members: each org submits their ed25519 public
   key. These are concatenated in canonical order and hashed to produce
   the set ID (see protocol spec, section "AttestorSet").
2. The threshold M-of-N for that set. Devnet default is roughly 2/3
   rounded up (so 3-of-5, 2-of-3).
3. Initial schemas to seed at genesis. For v0 that is one:
   `themisra.proof-of-prompt` version 1, owned by the Ligate Labs address,
   attestor set as above, `fee_routing_bps = 0`, `fee_routing_addr = None`.
4. Treasury address (receives all registration fees and the non-builder
   share of attestation fees).
5. Fee constants: `ATTESTOR_SET_FEE`, `SCHEMA_REGISTRATION_FEE`,
   `ATTESTATION_FEE`. Protocol defaults (10 / 100 / 0.001 $LGT) unless
   agreed otherwise on the call.

**Output**

A directory of per-module JSON files committed to this repo (under
`devnet/genesis/`), signed off on by all participating orgs. The
sequencer and every full node boot from an identical copy of those
files. A canonical example lives at
[`docs/development/genesis.example.json`](genesis.example.json) (the
attestation module's slice; the runbook example expands this set).

## 5. Running a node

The current `ligate-node` binary takes a `--da-layer` flag (`mock` or
`celestia`) and points at a rollup config TOML plus a genesis directory.
Per-flavour boot details live in
[`devnet/README.md`](../../devnet/README.md). The shapes below are the
intended **devnet operator** form, layered on top of that local-boot
flow with hardened defaults (persistent state dir, externalised auth
tokens, public RPC bind).

**Sequencer**

```bash
SOV_CELESTIA_RPC_URL=wss://celestia-mocha.public-rpc.com \
SOV_CELESTIA_SIGNER_KEY=$(pass celestia/devnet-signer) \
cargo run --release --bin ligate-node -- \
    --da-layer celestia \
    --rollup-config-path /etc/ligate/celestia.toml \
    --genesis-config-dir /etc/ligate/genesis
```

The `[storage]` section of `celestia.toml` controls the data
directory; CLI flags for it land in a future binary release.

**Attestor or full node**

A full node uses the same binary with the same args; it differs from
the sequencer only in **what's allowed to do at runtime**, which is
governed by genesis (`sequencer_registry.json` lists who can sequence,
`attester_incentives.json` who can attest). An attestor org runs the
node, signs blobs server-side via its quorum service, and submits
through the sequencer's RPC. See section 7 for key handling.

> **Future work.** A flag like `--attestor-key-file` is **not** part of
> the v0 binary. v0 attestors sign off-chain in their quorum service
> (their own infra, outside this repo) and the chain only verifies the
> resulting M-of-N signatures on `SubmitAttestation`. Bundling
> attestor-key handling into `ligate-node` is a v1+ design call,
> tracked alongside the staking module (#50).

**Health checks**

A healthy node logs roughly:

- `sequencer: produced block height=N da_height=M` (sequencer only)
- `da: submitted blob height=N size=B` (sequencer only)
- `attestation: validated N signatures, threshold met` on each
  `SubmitAttestation` it sees
- No sustained `da: submission failed` or `rpc: 5xx` lines

For programmatic liveness, the SDK auto-mounts `/sequencer/ready` and
`/rollup/sync-status` (see the SDK's [REST surface](https://github.com/Sovereign-Labs/sovereign-sdk)
for the full set; a curated reference for our deployed surface is the
follow-up tracked alongside #79).

## 6. Submitting a test attestation

The chain-side query API for verifying receipts landed in #93 (closed
read-side of #21). A dedicated `ligate-cli` operator tool that wraps
the steps below into one-line commands is **future work** (own issue
forthcoming). Today the steps execute via `crates/client-rs` typed
builders + a Rust caller, or by hitting the REST endpoints with `curl`.

The end-to-end flow for a single Themisra attestation on devnet:

1. **AttestorSet**. Use the set registered at genesis. No action needed.
   For a new schema with its own set, call
   `CallMessage::RegisterAttestorSet { members, threshold }` once, from
   the schema owner's address, paying `ATTESTOR_SET_FEE`.
2. **Schema**. Use `themisra.proof-of-prompt/v1` from genesis. For a new
   schema, call `CallMessage::RegisterSchema { name, version,
   attestor_set, fee_routing_bps, fee_routing_addr }` from the owner
   address, paying `SCHEMA_REGISTRATION_FEE`.
3. **Collect signatures**. The client SDK hashes the domain payload
   (Themisra: `borsh((model, sha256(prompt), sha256(output), timestamp,
   client))`) into a 32-byte `payload_hash`, then requests signatures
   from the off-chain quorum service. The quorum service asks each
   attestor to sign the canonical Borsh encoding of:

   ```
   SignedPayload {
       schema_id:    [u8; 32],
       payload_hash: [u8; 32],
       submitter:    [u8; 32],
       timestamp:    u64,
   }
   ```

   See protocol spec, section "Signed payload for attestations", for the
   exact bytes. Do not re-derive this informally: the Borsh encoding is
   load-bearing.

4. **Submit**. The client sends
   `CallMessage::SubmitAttestation { schema_id, payload_hash, signatures }`
   to the sequencer. Signatures must meet the schema's
   `AttestorSet.threshold` or the tx is rejected.
5. **Query**. Read the attestation back via REST:

   ```
   GET /modules/attestation/attestations/{schemaId}:{payloadHash}
   ```

   Returns the on-chain record: schema ID, payload hash, submitter,
   timestamp, validated signatures. Companion endpoints:
   `GET /modules/attestation/schemas/{schemaId}` and
   `GET /modules/attestation/attestor-sets/{attestorSetId}`. Wired in
   PR #93. Range / by-submitter / aggregation queries land in the
   indexer service (#91), not the node.

The `(schema_id, payload_hash)` pair is write-once. Resubmitting the same
pair is rejected at the state-transition layer (invariant 5 in the
protocol spec).

## 7. Attestor operator guide

**Key management**

- Generate the ed25519 keypair on a machine you control. Do not let the
  Ligate Labs sequencer generate it for you.
- Store the private key in a hardware security module (YubiHSM, AWS KMS,
  etc.) or at minimum on a LUKS/FileVault-encrypted disk, owned by a
  dedicated system user, mode 0600.
- Back up the private key offline (air-gapped USB, paper QR) before
  bringing the node online. If you lose it, you cannot recover attestor
  status in v0 (see "Key rotation" below).
- Never share the private key between orgs. The whole point of the
  federated set is independent compromise surface.

**Signing**

Attestors **do not construct attestation payloads**. They receive a
`SignedPayload` digest from the off-chain quorum service for their
schema and sign it with their ed25519 private key. Everything upstream
of the signature (what a valid Themisra attestation means, how the
quorum service decides to request one) is the schema owner's concern,
documented in the relevant product repo.

Do not accept digests from any source other than the authorised quorum
service endpoint your org agreed to at onboarding. Signing arbitrary
digests is equivalent to handing out blank-cheque attestations.

**Uptime**

- Devnet: **best effort**. If your node is offline for a few hours,
  other attestors in the set cover the threshold. If you are offline
  long enough that the set cannot meet its threshold, submissions stall.
- Mainnet (future): a formal SLA will be attached to attestor onboarding,
  and v1's `disputes` module will slash for persistent unavailability.
  That's not in scope here.

**Key rotation**

**Not supported in v0.** The protocol spec makes `AttestorSet` immutable
once registered (see spec section "AttestorSet"). To rotate an attestor's
key, the schema owner must:

1. Register a new `AttestorSet` containing the rotated key.
2. Register a new `Schema` version pointing to the new set.
3. Migrate clients to the new schema version.

If your private key is compromised, notify Ligate Labs immediately
(`hello@ligate.io`) so the schema owner can initiate a rotation.
Compromised keys cannot be revoked in place.

## 8. Monitoring and observability

Concrete dashboards and alert rules are pending. At minimum, every
operator should watch:

**Sequencer**

- Block production rate (blocks per minute).
- DA submission latency and DA submission failure rate.
- RPC error rate (5xx per minute).
- Mempool depth.

**Attestor or full node**

- Sync lag behind sequencer head.
- Attestation-submission success rate (valid signatures present, threshold
  met, tx accepted).
- Attestor signature-validation rejects (how often submissions arrive
  with signatures that don't verify against the registered set).

**Attestor-specific**

- Signing request rate from the quorum service.
- Local signing latency (time between digest received and signature
  emitted).
- Any unexpected signing requests (digests that did not come from the
  authorised quorum endpoint). Treat as a security event.

### Prometheus `/metrics` endpoint

`ligate-node` exposes a Prometheus text-format `/metrics` endpoint at
`http://127.0.0.1:9100/metrics` by default.

**Counters** (all cold-start zero, each increments per successful handler call):

- `ligate_attestor_sets_registered_total`
- `ligate_schemas_registered_total`
- `ligate_attestations_submitted_total`

**Rejection counter** (Phase 2 chunk in [#167](https://github.com/ligate-io/ligate-chain/pull/167)):

- `ligate_attestations_rejected_total{reason}` — bumps on any handler that returns an `AttestationError`. The `reason` label is the variant's stable snake_case discriminant (e.g. `unknown_schema`, `below_threshold`, `invalid_signature`); the full mapping is in [`error-coverage.md`](error-coverage.md).

**Gauges:**

- `ligate_state_db_size_bytes` — total on-disk size of the rollup's storage directory, sampled every 30 seconds. Captures RocksDB SST files, WAL, manifest, and the ledger DB.
- `ligate_block_height` — current rollup head slot number observed by this node, sampled every 2 seconds via the SDK's `LedgerDb::get_head_slot_number`. Sequencers report what they're producing; followers report what they've replayed from DA. Lag between two nodes scraping the same chain is the follower-vs-sequencer sync gap.

**RPC histograms** (axum middleware over the SDK's REST router):

- `ligate_rpc_requests_total{endpoint,status}` — counter, one bump per matched request. The `endpoint` label is the route template (`/modules/attestation/schemas/{schema_id}`), not the concrete `:id` value. Unmatched routes (404s on arbitrary paths) are skipped to keep cardinality bounded.
- `ligate_rpc_request_duration_seconds{endpoint}` — histogram, one observation per matched request, in seconds. Default Prometheus buckets (0.005s through 10s).

**Process metrics** (Linux only): `process_cpu_seconds_total`, `process_resident_memory_bytes`, `process_open_fds`, `process_max_fds`, `process_start_time_seconds`, `process_virtual_memory_bytes`. Auto-registered by the `prometheus` crate's `process` feature; macOS and Windows builds skip them.

```bash
# Default bind (localhost-only, safe for direct scraping in dev).
curl http://127.0.0.1:9100/metrics

# Behind a reverse proxy with operator-controlled exposure.
ligate-node --metrics-bind 127.0.0.1:9100 ...

# Disable the endpoint entirely.
ligate-node --metrics-bind off ...
```

Phase 2 expands the metric set (DA submission latency, mempool depth,
RPC histograms, state DB size) and ships a starter Grafana dashboard.
Tracked under follow-up issues filed alongside the Phase 1 PR.

## 9. Known limitations (devnet phase)

| Limitation | Tracking issue |
|---|---|
| No economic slashing (v1 feature) | #51 (disputes module, v1) |
| No `list_by_schema` REST endpoint (deferred from #21) | follow-up; deeper queries land in the indexer service |
| Hosted public RPC + sequencer infrastructure not stood up | #79 |
| Block explorer not deployed | #80 |
| Devnet faucet not deployed | #95 |
| Force-include path (sequencer-bypass) not designed | #81 (v1) |
| Multi-sequencer / leader rotation | #82 (v1) |
| Block indexer service (Postgres + ingest) | #91 |
| WebSocket / SSE live event stream | #92 |
| Full Prometheus metric set + Grafana dashboard | follow-up to #110 |
| `ligate-cli` operator tool | not yet filed |

The chain itself is live: `Schema`, `AttestorSet`, `Attestation`, the
`CallMessage` enum, state transitions, ed25519 signature validation,
fee routing, runtime composition, Celestia DA, and the Risc0 inner
zkVM are all implemented against Sovereign SDK rev
`f89964c66bcfb6a31712e43278378b54c33d3779`. The gaps above are
**around** the chain: hosted infrastructure, ops tooling, indexing,
and the v1 economic-trust module set.

## 10. Getting help

- **Bugs, questions, issues with this document**: open a GitHub issue on
  `github.com/ligate-io/ligate-chain`.
- **Interested in operating a devnet attestor**: email `hello@ligate.io`.
  We are actively looking for 3 to 5 orgs for the v0 set.
