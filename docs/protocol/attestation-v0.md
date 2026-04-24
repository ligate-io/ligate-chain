# Ligate Attestation Protocol — Specification v0

**Status:** design draft for v0 devnet implementation
**Scope:** types, state layout, state transitions, fees, invariants
**Applies to:** the `attestation` Sovereign SDK module in `crates/modules/attestation`

---

## Purpose

Ligate Chain's core primitive is a **generic on-chain attestation protocol**.

Anyone can:

1. **Register an AttestorSet** — a quorum of signers (pubkeys + threshold)
2. **Register a Schema** — the shape of an attestation: who owns it, what it's called, which AttestorSet must sign under it, optional fee routing
3. **Submit an Attestation** — a signed payload hash bound to a schema

Chain stores only hashes and signatures — never plaintext prompts, outputs, or sensitive payload data. Apps hash their domain-specific payloads client-side.

The protocol is intentionally narrow. It does **not** do identity, reputation, slashing, cryptographic inference verification, or payments. Those are separate modules or external primitives composed on top.

**Flagship consumers** (using the protocol; not part of it):
- **Themisra** — Proof of Prompt (schema: `themisra.proof-of-prompt/v1`)
- **Kleidon** — SaaS / gaming products (multiple schemas in v1+)
- **Ligate MCP Server + Relayer** — attestation for AI agents (multiple schemas, v0.5 product)

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
    fee_routing_bps:  u16               // 0–5000 basis points (cap 50%)
    fee_routing_addr: Option<Address>   // where the builder's share goes; None = 100% treasury
}
```

Registration is **permissionless** (anyone can register) — spam mitigated by the registration fee in $LGT.

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

**Deliberately omitted:** `VerifyReceipt` / `VerifyAttestation`. Verification is a read — exposed as RPC query, not a state-mutating transaction.

---

## Query RPC (read path)

```
get_schema(schema_id)                       -> Option<Schema>
get_attestor_set(set_id)                    -> Option<AttestorSet>
get_attestation(schema_id, payload_hash)    -> Option<Attestation>
list_attestations_by_schema(schema_id, pagination)  -> Vec<AttestationKey>
list_schemas_by_owner(owner, pagination)    -> Vec<SchemaId>
```

No transactions. No state changes. Cheap to serve from indexed node.

---

## Invariants

The state transitions MUST enforce:

1. `RegisterSchema.attestor_set` references a registered `AttestorSet` — else reject.
2. `RegisterSchema.fee_routing_bps` ≤ `MAX_BUILDER_BPS` (default 5000 = 50%, governance-adjustable in v1+) — else reject.
3. `RegisterSchema.fee_routing_addr.is_some() == (fee_routing_bps > 0)` — no dangling routing, no orphan addr.
4. `SubmitAttestation` signatures must:
   - All `pubkey`s be ∈ schema's `AttestorSet.members`
   - All signatures verify over canonical Borsh encoding of the signed payload (spec'd below)
   - Count of valid signatures ≥ schema's `AttestorSet.threshold`
5. `(schema_id, payload_hash)` is write-once — `SubmitAttestation` for an existing pair is rejected.
6. `SchemaId` derivation must match `SHA-256(owner ‖ name ‖ version)` exactly; redundant — but recomputed by the runtime at registration and stored on-chain so consumers don't have to trust the submitter's claimed ID.

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

Signatures do NOT cover the `signatures` field itself (that would be circular). Replay is prevented by the `(schema_id, payload_hash)` write-once invariant — the same pair can't be submitted twice, so the same signature can't be replayed.

---

## Fees

All amounts in $LGT.

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

Schema owners choose `fee_routing_bps` at registration (0–5000). Changing requires a schema version bump. Users of the schema implicitly opt in by migrating to new versions — no bait-and-switch possible on existing users.

---

## Permissionless-by-design

- Anyone can `RegisterAttestorSet` with any set of pubkeys — quality is a social concern, not a protocol gate.
- Anyone can `RegisterSchema` with any name — different owners can register the same `name` (different `schema_id`s). Social layer handles identity / "verified" tags.
- Anyone can `SubmitAttestation` under any schema, provided they supply valid signatures from that schema's `AttestorSet`. The `AttestorSet` is the permission layer.

---

## Non-goals for v0

The protocol intentionally does not handle:

- **Identity** — v1 module `identity` adds DID-like lookups (address → handle, pubkey, metadata URI).
- **Reputation / slashing** — v1 module `disputes` adds on-chain flagging + attestor slashing tied to AttestorSet stake.
- **Cryptographic inference verification** — v2+ schemas like `themisra.verified-inference/v1` carry external zkML / TEE proof hashes in their `payload_hash`; clients verify the external proof off-chain with the proving system that generated it. **Ligate does not build its own zkML.**
- **Payments / streaming / metered billing** — v2 module `payments` extends `bank`.
- **Agent registry** — v2 module `agents` (distinct from the v0.5 Ligate MCP Server, which is a developer-facing tool on top of this protocol, not a new module).

---

## Trust model

**v0 is federated.** Attestor sets are operated by the schema owner (for Themisra, by Ligate Labs; for third-party schemas, by whoever registers them). Correctness depends on the quorum being trustworthy.

**v1 adds economic skin** via slashing in the `disputes` module.

**v2 optionally adds cryptographic proof** by letting schemas carry zkML/TEE proof hashes in their payloads. Ligate stays neutral on proving tech — customers bring their own.

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

The attestor quorum HTTP API, the Themisra payload canonicalisation, and the TypeScript/Rust SDK surfaces are out of scope for this chain-level spec — they're Themisra product concerns, documented in the `themisra-pop` crate and the `@ligate/themisra` package.
