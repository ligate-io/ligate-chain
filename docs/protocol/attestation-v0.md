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
    id:                  [u8; 32]          // SHA-256(owner ‖ name ‖ version)
    owner:               Address           // who registered it; can upgrade (new version = new id)
    name:                String            // human-readable, e.g. "themisra.proof-of-prompt"
    version:             u32               // monotonic; immutable per id
    attestor_set:        AttestorSetId     // which quorum must sign under this schema
    fee_routing_bps:     u16               // 0-5000 basis points (cap 50%)
    fee_routing_addr:    Option<Address>   // where the builder's share goes; None = 100% treasury
    payload_shape_hash:  [u8; 32]          // off-chain payload-spec content-address; stored not verified
}
```

Registration is **permissionless** (anyone can register), spam mitigated by the registration fee in AVOW.

**Namespace ownership:** `schema_id` is derived from `owner` too, so two different owners registering the same `name` produce different `schema_id`s. No protocol-level name squatting. Social identity (who is the "real" Themisra) is handled off-chain via owner address + verified-tag metadata in SDKs and explorers. Same model as ERC-20 token names.

**`payload_shape_hash`:** 32-byte content-address of the off-chain payload spec that `payload_hash` is computed against. **The chain stores this value but does not verify it.** The shape spec lives off-chain, so the chain has no view of the relationship between `payload_hash` and `payload_shape_hash`; storing the hash without the data it refers to is documentation, not enforcement. The field exists for multi-SDK drift protection: any SDK whose computed payload-format hash differs from the registered value is non-conformant. Pass `[0u8; 32]` to opt out, the genesis loader and `RegisterSchema` handler treat that value as "no off-chain spec association". See [#151](https://github.com/ligate-io/ligate-chain/issues/151) for the rationale.

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
        name:               String,
        version:            u32,
        attestor_set:       AttestorSetId,
        fee_routing_bps:    u16,
        fee_routing_addr:   Option<Address>,
        payload_shape_hash: [u8; 32],
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
GET /modules/attestation/attestations/{attestation_id}             -> Attestation | 404
```

Wired in [PR #93](https://github.com/ligate-io/ligate-chain/pull/93) (closes the read-side of [#21](https://github.com/ligate-io/ligate-chain/issues/21)). 400 on malformed Bech32, 404 on missing key, 200 with typed JSON body otherwise.

`attestation_id` is a single bech32m string with the `lat` HRP (`lat1…`), derived as `SHA-256(schema_id_bytes ‖ payload_hash_bytes)`. Pre-v0.2.0 used the compound `<schema_id>:<payload_hash>` (`lsc1…:lph1…`) form; the v0.2.0 cut collapsed it to a single bech32 string for consistency with `lsc`/`las`/`lph`/`lpk`. The schema and payload-hash components remain queryable from the returned `Attestation` body.

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

Attestors sign `SHA-256(borsh(SignedAttestationPayload))` where:

```rust
struct SignedAttestationPayload<S: Spec> {
    schema_id:    SchemaId,    // [u8; 32]
    payload_hash: PayloadHash, // [u8; 32]
    submitter:    S::Address,  // MultiAddress<VmAddress>, enum
    timestamp:    u64,
}
```

The signature is verified by the chain in `handle_submit_attestation`; if your off-chain digest doesn't match the chain's, the chain now surfaces the exact inputs it used (see `AttestationError::InvalidSignature`).

### Wire format (read this if you're rolling your own signer)

The Borsh encoding is the bytes that get SHA-256'd. **Total: 101 bytes** (NOT 100, see discriminator below):

| Offset | Length | Field           | Notes |
|--------|--------|-----------------|-------|
| 0      | 32     | `schema_id`     | Raw 32 bytes (Borsh of `[u8; 32]` is the raw array, no length prefix). |
| 32     | 32     | `payload_hash`  | Raw 32 bytes. |
| 64     | 1      | submitter tag   | `0x00` for `MultiAddress::Standard`. **Easy to miss.** |
| 65     | 28     | submitter addr  | The 28-byte `Address` payload inside `Standard`. |
| 93     | 8      | `timestamp`     | u64 little-endian. Pinned to `0x0000000000000000` in v0 (chain hardcodes `timestamp = 0` until the runtime exposes block headers; tracked in [ligate-chain#TBD]). |

The submitter byte is the most common stumble. `S::Address` resolves through `MockRollupSpec` to `MultiAddress<VmAddress>`, a two-variant enum (`Standard(Address)`, `Vm(VmAddress)`). Borsh prepends a 1-byte discriminator before the variant payload. For chain-native `lig1...` addresses you always want the `0x00` `Standard` variant.

### Test vector

Inputs (the exact values are also baked into the `attestation_digest` rustdoc as a doctest, so this stays in sync with the impl):

| Field          | Value |
|----------------|-------|
| `schema_id`    | `lsc1zg3qygc(...)`, raw bytes `0x1c24...9486` |
| `payload_hash` | `lph1he42sgwn(...)`, raw bytes `0xbe6a...13ac` |
| `submitter`    | `lig1zd9j2z6x55ydnv9m8f0pdw3vs2j8u0w5sdqeaf478dzp6s998ac`, raw bytes `0x134b...441d` |
| `timestamp`    | `0` |

Borsh bytes (101 bytes, hex):

```
1c24a84b8307ff2a9e859218d76476932555b5214f8c1c555224b620f8b19486
be6aa821d3f6c3405edcb7cddbcf419e00119e321c6fb46452281dafd55913ac
00
134b250b46a508d9b0bb3a5e16ba2c82a47e3dd483419ea6be3b441d
0000000000000000
```

Expected digest (SHA-256 of the above):

```
b26c0c1698c07e97f3f426ccbdc61ae16dee6f13b780d59c612c7c2b6ba3a079
```

If you compute the digest in your off-chain signer and get something different, the most likely cause is the missing `0x00` byte before the submitter address. The next most likely is the timestamp not being 8 little-endian bytes (Borsh `u64` is always 8 LE).

### Helpers

Don't roll your own if you can avoid it. Use:

- **Rust:** `ligate_client::attestation_digest::<S>(schema_id, payload_hash, submitter, timestamp)` + `ligate_client::sign_attestation(signing_key, &digest)`.
- **CLI:** `ligate sign-attestation --schema <lsc1> --payload-hash <lph1> --submitter <lig1> --signer <role>` produces a ready-to-submit signature entry.
- **TypeScript:** `@ligate/js`'s `signAttestation(...)` (same byte layout, verified against the Rust path via the cross-impl test in `packages/js/test/attestation-vector.test.ts`).

### Replay

Signatures don't cover the `signatures` field of `Attestation` (that would be circular). Replay is prevented by the `(schema_id, payload_hash)` write-once invariant: the same pair can't be submitted twice, so the same signature can't be replayed.

---

## Fees

All amounts in AVOW.

`AVOW` uses **9 decimals** on the wire. The smallest representable unit is `1 nano` = `0.000000001 AVOW`. Genesis configs and SDK call sites carry amounts as `u64` base units (nanos); user-facing UIs format as decimal `AVOW`. The 9-decimal choice mirrors Solana (lamport) and Cosmos micro-precision conventions while leaving 18.4x headroom over a 1B-supply cap inside `u64`, the amount type `sov-bank` enforces.

| Event | Fee | Split |
|---|---|---|
| `RegisterAttestorSet` | `ATTESTOR_SET_FEE` (governance; devnet-1 genesis 0.05 AVOW) | 100% treasury |
| `RegisterSchema` | `SCHEMA_REGISTRATION_FEE` (governance; devnet-1 genesis 0.1 AVOW) | 100% treasury |
| `SubmitAttestation` | `ATTESTATION_FEE` (governance; devnet-1 genesis 0.0001 AVOW) | See below |

Devnet-1 genesis values live in `devnet-1/genesis/attestation.json` and are stored in nano-AVOW (9 decimals); e.g. `attestation_fee = "100000"` nano = 0.0001 AVOW.

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
