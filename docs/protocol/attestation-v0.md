# Ligate Attestation Protocol, Specification v0

**Status:** design draft for v0 devnet implementation
**Scope:** types, state layout, state transitions, fees, invariants
**Applies to:** the `attestation` Sovereign SDK module in `crates/modules/attestation`

---

## Purpose

Ligate Chain's core primitive is a **generic on-chain attestation protocol**.

Anyone can:

1. **Register an AttestorSet**, a quorum of signers (pubkeys + threshold)
2. **Register a Schema**, the shape of an attestation: who owns it, what it's called, which AttestorSet must sign under it, optional fee routing
3. **Submit an Attestation**, a signed payload hash bound to a schema

Chain stores only hashes and signatures, never plaintext prompts, outputs, or sensitive payload data. Apps hash their domain-specific payloads client-side.

The protocol is intentionally narrow. It does **not** do identity, reputation, slashing, cryptographic inference verification, or payments. Those are separate modules or external primitives composed on top.

**Flagship consumers** (using the protocol; not part of it):
- **Themisra**, Proof of Prompt (schema: `themisra.proof-of-prompt/v1`)
- **Iris**, attestation for AI agents (MCP server + relayer; multiple schemas; v0.5 product)
- **Kleidon**, SaaS / gaming products (multiple schemas in v1+)

---

## Positioning

Ligate Chain is a **specialized app-chain**, not a general-purpose smart-contract platform. The protocol described here is the chain's core primitive, not the only thing the chain does, but it is the only thing **users program**.

There is no Solidity, no Vyper, no CosmWasm, no contract deployment via tx in v0 through v3. New chain capabilities arrive as new modules through coordinated upgrades, not as user-deployed code. Fungible tokens and NFTs, when they ship in v1, come from curated modules (`tokens`, `nft`), not from arbitrary contract execution.

EVM compatibility is tracked as a long-horizon v4 option (chain issue #52). It is deliberately deferred until the attestation thesis has shipped and proven out. Pre-mainnet readers should treat this chain as non-EVM; treat any earlier opening of EVM compatibility as a strategic shift that requires its own announcement, not a feature add.

Permissionless extension exists at exactly one layer: anyone can register a `Schema` plus an `AttestorSet` on the `attestation` module and submit attestations under their own rules. That is the entire user-facing surface for application logic.

This is deliberate. The right comparisons are Celestia (specialised for DA), Hyperliquid (perps), dYdX v4 (derivatives), or Cosmos app-chains, not Ethereum or general L1s. The chain is shaped for attestation infrastructure plus a small set of sister modules; building a DEX or a lending protocol on top is not in scope and never will be.

---

## Entities

### Schema

Identifies an app's attestation shape and rules.

```
Schema {
    id:               [u8; 32]          // SHA-256(owner ‖ name ‖ version)
    owner:            Address           // who registered it; can upgrade (new version = new id)
    name:             String            // human-readable, e.g. "themisra.proof-of-prompt"
    version:          u32               // monotonic; immutable per id
    attestor_set:     AttestorSetId     // which quorum must sign under this schema
    fee_routing_bps:  u16               // 0-5000 basis points (cap 50%)
    fee_routing_addr: Option<Address>   // where the builder's share goes; None = 100% treasury
}
```

Registration is **permissionless** (anyone can register), spam mitigated by the registration fee in $LGT.

**Namespace ownership:** `schema_id` is derived from `owner` too, so two different owners registering the same `name` produce different `schema_id`s. No protocol-level name squatting. Social identity (who is the "real" Themisra) is handled off-chain via owner address + verified-tag metadata in SDKs and explorers. Same model as ERC-20 token names.

### AttestorSet

A quorum of signers. **Immutable once registered.**

```
AttestorSet {
    id:         [u8; 32]        // SHA-256(members ‖ threshold)
    members:    Vec<PubKey>     // ed25519 for v0
    threshold:  u8              // M-of-N signatures required
}
```

To "rotate" an attestor set, register a new one and point an affected schema to it via a schema version bump. No in-place mutation.

### Attestation

The on-chain record.

```
Attestation {
    schema_id:    SchemaId
    payload_hash: [u8; 32]                // app-computed; chain never sees plaintext
    submitter:    Address
    timestamp:    u64                      // unix seconds, from the tx's block header
    signatures:   Vec<AttestorSignature>  // validated against schema.attestor_set at submit time
}

AttestorSignature {
    pubkey: [u8; 32]      // must be ∈ AttestorSet.members
    sig:    Vec<u8>       // scheme-dependent length (ed25519 = 64, BLS = 48)
}
```

---

## State

Three `StateMap`s in the `attestation` module:

| Map | Key | Value | Notes |
|---|---|---|---|
| `schemas` | `SchemaId` | `Schema` | set once, immutable after |
| `attestor_sets` | `AttestorSetId` | `AttestorSet` | set once, immutable after |
| `attestations` | `(SchemaId, PayloadHash)` | `Attestation` | write-once per pair |

---

## Call messages

```
enum CallMessage {
    RegisterAttestorSet {
        members:   Vec<PubKey>,
        threshold: u8,
    },
    RegisterSchema {
        name:             String,
        version:          u32,
        attestor_set:     AttestorSetId,
        fee_routing_bps:  u16,
        fee_routing_addr: Option<Address>,
    },
    SubmitAttestation {
        schema_id:    SchemaId,
        payload_hash: [u8; 32],
        signatures:   Vec<AttestorSignature>,
    },
}
```

**Deliberately omitted:** `VerifyReceipt` / `VerifyAttestation`. Verification is a read, exposed as RPC query, not a state-mutating transaction.

---

## Query RPC (read path)

The chain's REST API is **point lookups only by design**. Three endpoints, all single-key:

```
GET /modules/attestation/schemas/{schema_id}                       -> Schema | 404
GET /modules/attestation/attestor-sets/{attestor_set_id}           -> AttestorSet | 404
GET /modules/attestation/attestations/{schema_id}:{payload_hash}   -> Attestation | 404
```

Wired in [PR #93](https://github.com/ligate-io/ligate-chain/pull/93) (closes the read-side of [#21](https://github.com/ligate-io/ligate-chain/issues/21)). 400 on malformed Bech32, 404 on missing key, 200 with typed JSON body otherwise.

### What's deliberately not here

List, range, time-bucketed, by-submitter, aggregation, top-N, search, full-text — none of those are on the chain. Three reasons:

1. **The chain's data is keyed for verification, not for search.** Every node would have to scan the full attestations map to satisfy a `list_by_schema` query, which is microseconds at v0 devnet volume and minutes at production volume. Pushing search work into the consensus path makes every node pay for every random user's query.
2. **Adding a secondary index is a hard fork.** A `StateMap<SchemaId, _>` populated on the write path would change the on-chain state shape — see [`docs/protocol/upgrades.md`](upgrades.md) for what counts as a hard fork. We don't want speculative hard forks for query patterns that have a better home elsewhere.
3. **There's a better home.** Range / list / aggregation queries belong in a separate **indexer service** ([#91](https://github.com/ligate-io/ligate-chain/issues/91)) that reads the chain's REST + WebSocket surface and writes into a real database (Postgres). Standard pattern across every mature chain — Cosmos has Numia / SubQuery / Mintscan, Ethereum has The Graph / Goldsky / Alchemy, Solana has Helius / Triton, Aptos / Sui ship official indexers alongside their validators. The chain stays narrow and fast; the indexer absorbs query-pattern complexity.

If a future query pattern can be answered with a single direct lookup against existing state (e.g. `get_schema_owner_count` derived as a counter on the schema entry), it's fair game for the chain. If it requires scanning, sorting, joining, or windowing, it belongs in the indexer — full stop.

### What lives in the indexer instead

Tracked in [#91](https://github.com/ligate-io/ligate-chain/issues/91), shipping pre-public-devnet alongside the explorer ([#80](https://github.com/ligate-io/ligate-chain/issues/80)):

- `list_attestations_by_schema(schema_id, from?, to?, page, limit)` — the canonical example
- `list_schemas_by_owner(owner_address, page, limit)`
- `list_attestations_by_submitter(address, page, limit)`
- Time-range queries, top-N stats, full-text search
- WebSocket subscriptions for live events ([#92](https://github.com/ligate-io/ligate-chain/issues/92))

These read the chain's point-lookup REST + the future event firehose; they do not require any chain-side change. The boundary holds.

---

## Invariants

The state transitions MUST enforce:

1. `RegisterSchema.attestor_set` references a registered `AttestorSet`, else reject.
2. `RegisterSchema.fee_routing_bps` ≤ `MAX_BUILDER_BPS` (default 5000 = 50%, governance-adjustable in v1+), else reject.
3. `RegisterSchema.fee_routing_addr.is_some() == (fee_routing_bps > 0)`, no dangling routing, no orphan addr.
4. `SubmitAttestation` signatures must:
   - All `pubkey`s be ∈ schema's `AttestorSet.members`
   - All signatures verify over canonical Borsh encoding of the signed payload (spec'd below)
   - Count of valid signatures ≥ schema's `AttestorSet.threshold`
5. `(schema_id, payload_hash)` is write-once, `SubmitAttestation` for an existing pair is rejected.
6. `SchemaId` derivation must match `SHA-256(owner ‖ name ‖ version)` exactly; redundant, but recomputed by the runtime at registration and stored on-chain so consumers don't have to trust the submitter's claimed ID.

---

## Signed payload for attestations

Attestors sign the canonical Borsh encoding of:

```
SignedPayload {
    schema_id:    [u8; 32],
    payload_hash: [u8; 32],
    submitter:    [u8; 32],
    timestamp:    u64,
}
```

Signatures do NOT cover the `signatures` field itself (that would be circular). Replay is prevented by the `(schema_id, payload_hash)` write-once invariant, the same pair can't be submitted twice, so the same signature can't be replayed.

---

## Fees

All amounts in $LGT.

`$LGT` uses **9 decimals** on the wire. The smallest representable unit is `1 nano` = `0.000000001 $LGT`. Genesis configs and SDK call sites carry amounts as `u64` base units (nanos); user-facing UIs format as decimal `$LGT`. The 9-decimal choice mirrors Solana (lamport) and Cosmos micro-precision conventions while leaving 18.4x headroom over a 1B-supply cap inside `u64`, the amount type `sov-bank` enforces.

| Event | Fee | Split |
|---|---|---|
| `RegisterAttestorSet` | `ATTESTOR_SET_FEE` (governance, default 10 $LGT) | 100% treasury |
| `RegisterSchema` | `SCHEMA_REGISTRATION_FEE` (governance, default 100 $LGT) | 100% treasury |
| `SubmitAttestation` | `ATTESTATION_FEE` (governance, default 0.001 $LGT) | See below |

### Attestation fee split

```
total = ATTESTATION_FEE
builder = total × schema.fee_routing_bps / 10000   (if fee_routing_addr set, else 0)
treasury = total - builder
```

Builder's share is sent to `schema.fee_routing_addr`. Treasury receives the rest.

Schema owners choose `fee_routing_bps` at registration (0-5000). Changing requires a schema version bump. Users of the schema implicitly opt in by migrating to new versions, no bait-and-switch possible on existing users.

---

## Permissionless-by-design

- Anyone can `RegisterAttestorSet` with any set of pubkeys, quality is a social concern, not a protocol gate.
- Anyone can `RegisterSchema` with any name, different owners can register the same `name` (different `schema_id`s). Social layer handles identity / "verified" tags.
- Anyone can `SubmitAttestation` under any schema, provided they supply valid signatures from that schema's `AttestorSet`. The `AttestorSet` is the permission layer.

---

## Non-goals for v0

The protocol intentionally does not handle:

- **Identity**, v1 module `identity` adds DID-like lookups (address → handle, pubkey, metadata URI).
- **Reputation / slashing**, v1 module `disputes` adds on-chain flagging + attestor slashing tied to AttestorSet stake.
- **Cryptographic inference verification**, v2+ schemas like `themisra.verified-inference/v1` carry external zkML / TEE proof hashes in their `payload_hash`; clients verify the external proof off-chain with the proving system that generated it. **Ligate does not build its own zkML.**
- **Payments / streaming / metered billing**, v2 module `payments` extends `bank`.
- **Agent registry**, v2 module `agents` (distinct from the v0.5 Ligate MCP Server, which is a developer-facing tool on top of this protocol, not a new module).

---

## Trust model

**v0 is federated.** Attestor sets are operated by the schema owner (for Themisra, by Ligate Labs; for third-party schemas, by whoever registers them). Correctness depends on the quorum being trustworthy.

**v1 adds economic skin** via slashing in the `disputes` module.

**v2 optionally adds cryptographic proof** by letting schemas carry zkML/TEE proof hashes in their payloads. Ligate stays neutral on proving tech, customers bring their own.

For the full v0 attacker model, every protection mechanism with its file location, and the list of risks deliberately out of scope, see [`threat-model.md`](threat-model.md).

---

## How Themisra consumes this

Themisra registers exactly one schema for v0:

```
name:             "themisra.proof-of-prompt"
version:          1
attestor_set:     <ligate-operated attestor set id>
fee_routing_bps:  0                   // all fees to treasury at launch
fee_routing_addr: None
```

Themisra's client SDK (Rust + TypeScript, shipping in `crates/schemas/themisra-pop` and npm package `@ligate/themisra`):

```
themisra.attest(model, prompt, output):
    payload      = borsh((model, sha256(prompt), sha256(output), timestamp, client))
    payload_hash = sha256(payload)
    signatures   = fetch from Themisra attestor quorum (off-chain; quorum HTTP API)
    chain.SubmitAttestation { THEMISRA_SCHEMA_ID, payload_hash, signatures }
    return receipt_id = (THEMISRA_SCHEMA_ID, payload_hash)

themisra.verify(receipt_id):
    attestation = chain.get_attestation(receipt_id)
    // caller validates whatever domain-specific checks they care about
    return attestation
```

The attestor quorum HTTP API, the Themisra payload canonicalisation, and the TypeScript/Rust SDK surfaces are out of scope for this chain-level spec, they're Themisra product concerns, documented in the `themisra-pop` crate and the `@ligate/themisra` package.

## Chain id

Cosmos-style string identifiers, locked to a single canonical ladder so every config file, doc, and frontend agrees on what to write:

| Environment                | Chain id           |
| -------------------------- | ------------------ |
| Local single-node dev      | `ligate-localnet`  |
| Public devnet              | `ligate-devnet-1`  |
| Public testnet (later)     | `ligate-testnet-1` |
| Mainnet (later)            | `ligate-1`         |

The trailing number bumps **only** on a chain restart that breaks state continuity (full reset, new genesis). Soft upgrades via the upgrade module ([#42](https://github.com/ligate-io/ligate-chain/issues/42)) keep the same id. The full soft-fork-vs-hard-fork story (when a change actually requires a chain-id bump, what the `CHAIN_HASH` guard catches automatically, and how the pre-mainnet vs post-mainnet upgrade flow differs) lives in [`upgrades.md`](upgrades.md).

The `0x` prefix is **never** correct on a chain id. Chain ids are bare strings, not hex.

The EIP-155 numeric chain id (the integer EVM tooling uses) is a separate concept and is not assigned in v0–v3. If we ever reserve one, potentially as part of the v4 EVM track ([#52](https://github.com/ligate-io/ligate-chain/issues/52)), it lives alongside the string id and does not replace it. The `CHAIN_ID = 4242` constant in [`constants.toml`](../../constants.toml) is the SDK's tx-signing domain-separation integer, not an EIP-155 id.

A locked convention plus a mechanical sweep across configs, docs, and frontends is tracked in [#54](https://github.com/ligate-io/ligate-chain/issues/54).
