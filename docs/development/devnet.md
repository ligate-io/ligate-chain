# Ligate Chain v0 Devnet Operator Runbook

**Status:** planning document for the v0 devnet. Several sections describe
capability that is not yet wired into the repo. Those sections are marked
**Preview only** and link to the tracking issues.

This document is **operations**. For the on-chain protocol (types, state,
invariants, fees), read [`docs/protocol/attestation-v0.md`](../protocol/attestation-v0.md)
first.

---

## 1. Overview

Ligate Chain v0 devnet is a **federated attestation testnet**:

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

> **Preview only.** The node binary in `crates/rollup/src/main.rs` is
> currently a placeholder. Real runtime wiring is tracked in #13 and the
> Celestia DA adapter in #2. The steps below describe the intended
> environment once that work lands. Do not expect any of the commands in
> sections 5 and 6 to execute today.

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

> **Preview only.** Genesis config loading is not implemented. Tracked in
> #8. Everything in this section describes the intended devnet launch
> process, not something you can run today.

The genesis ceremony is a one-time coordinated event to produce a shared
`genesis.json`.

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

A `genesis.json` committed to this repo (exact path pending #8), signed
off on by all participating orgs. The sequencer and every attestor node
boot from an identical copy of that file.

## 5. Running a node

> **Preview only.** The `ligate-node` binary does not exist yet. Command
> shapes below are placeholders to illustrate the intended interface.
> Tracked in #13.

**Sequencer**

```bash
# placeholder shape, not a runnable command
ligate-node sequencer \
    --config   /etc/ligate/genesis.json \
    --data-dir /var/lib/ligate \
    --da-endpoint http://127.0.0.1:26658 \
    --da-auth-token "$CELESTIA_AUTH_TOKEN" \
    --rpc-addr 0.0.0.0:8545
```

**Attestor or full node**

```bash
# placeholder shape, not a runnable command
ligate-node full \
    --config   /etc/ligate/genesis.json \
    --data-dir /var/lib/ligate \
    --sequencer-rpc https://rpc.devnet.ligate.io \
    --attestor-key-file /etc/ligate/attestor.key \
    --rpc-addr 127.0.0.1:8545
```

The `--attestor-key-file` flag is only meaningful if the node is
participating as an attestor. A plain full node watches state without
signing.

**Health checks**

Once the binary lands, a healthy node logs roughly:

- `sequencer: produced block height=N da_height=M` (sequencer only)
- `da: submitted blob height=N size=B` (sequencer only)
- `attestation: validated N signatures, threshold met` on each
  `SubmitAttestation` it sees
- No sustained `da: submission failed` or `rpc: 5xx` lines

A minimal `GET /health` endpoint on the node RPC will return 200 when the
node is caught up to DA and able to serve queries. Exact response shape
pending #21.

## 6. Submitting a test attestation

> **Preview only.** There is no `ligate-cli` today, and the Rust client
> in `crates/client-rs` is a scaffold. The flow below is the intended
> shape for devnet. Once the RPC query layer (#21) and node binary (#13)
> land, we will ship a CLI that implements these steps.

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
5. **Query**. Read the attestation back via RPC:

   ```
   attestation_getAttestation(schema_id, payload_hash) -> Option<Attestation>
   ```

   Returns the on-chain record: schema ID, payload hash, submitter,
   timestamp, validated signatures.

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

The node will expose Prometheus metrics once #13 lands. Grafana
dashboards will live under `ops/grafana/` in this repo. Both are TBD.

## 9. Known limitations (devnet phase)

| Limitation                          | Tracking issue |
|-------------------------------------|----------------|
| No economic slashing (v1 feature)   | v1 `disputes` module, not filed yet |
| Fee routing enforcement not wired   | #22 |
| No query RPC for attestations       | #21 |
| Node binary is a placeholder        | #13 |
| Celestia DA adapter not wired       | #2  |
| Genesis config loader not written   | #8  |

The attestation module itself (`crates/modules/attestation`) is live:
`Schema`, `AttestorSet`, `Attestation`, the `CallMessage` enum, state
transitions, and ed25519 signature validation are all implemented
against Sovereign SDK rev `13e4077c329ff14954b32e3180d43a6d86fa3172`.
The gap is everything around it: the runnable node, DA, genesis, RPC,
fee routing. See the issues above for current work.

## 10. Getting help

- **Bugs, questions, issues with this document**: open a GitHub issue on
  `github.com/ligate-io/ligate-chain`.
- **Interested in operating a devnet attestor**: email `hello@ligate.io`.
  We are actively looking for 3 to 5 orgs for the v0 set.
