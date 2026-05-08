# Canonical schema specs

This directory holds the canonical JSON Schema documents for first-party Ligate schemas (Themisra, Mneme, Iris, Kleidon products). Each file fully specifies the payload shape attestor quorums sign over for a given `(name, version)` pair.

## Files

| File | Schema | Status |
|---|---|---|
| `themisra.proof-of-prompt-v1.json` | `themisra.proof-of-prompt` v1.0.0 | v1 (devnet) |

Future first-party schemas land here. Third-party schemas can host their JSON Schema docs anywhere (the chain only stores the hash); we host first-party schemas in-tree so partners can pin against a versioned, public file.

## What's a `payload_shape_hash`?

The `attestation` module on-chain stores a `payload_shape_hash: [u8; 32]` per registered schema. It's an opaque commitment — the chain treats it as a black box and never validates a payload against the schema directly. The off-chain quorum service is the one that validates `(payload, schema_doc)` and refuses to sign anything that doesn't match.

So the contract between the chain and the off-chain world is: **everyone agrees on which JSON file is "the schema doc," and the `payload_shape_hash` registered on-chain is the SHA-256 of that file's bytes.**

### Canonicalization

To make the hash stable across editors / OSes / mirrors, the canonical form is the file's bytes verbatim, with these constraints baked in via `prettier`:

- **Encoding**: UTF-8, no BOM
- **Line endings**: LF (`\n`), no CRLF
- **Trailing newline**: exactly one (Unix convention; `prettier` enforces)
- **Indentation**: 2 spaces
- **Property order**: as authored (JSON Schema doesn't require alphabetization; semantic diff readability wins)
- **No trailing whitespace on any line**

The `cargo run -p ligate-bootstrap-cli -- compute-shape-hash <file>` command applies the canonicalization and prints the hash, so the operator never has to compute it by hand. The CI gate ensures the file's checked-in form already matches its canonical form.

## Versioning

Schemas are immutable once registered on-chain. To revise a schema, register `<name>` `v2` from the same owner and let consumers migrate. The original `v1` registration stays accessible for historical attestations.

The on-chain `schema_id` is `SHA-256(owner_addr || name || version)`, so changing any of those three fields produces a fresh `schema_id` (a fresh row in chain state).

## Spec status notes

- `themisra.proof-of-prompt-v1.json` is v1 of LIP-5 (#185). Sufficient for devnet and design-partner integrations. LIP-5 may iterate the spec; iterations ship as v2+ rather than mutating v1.

## Related

- [`docs/development/canonical-schema-registration.md`](../../development/canonical-schema-registration.md) — operator runbook for registering these schemas after devnet boot.
- [`docs/protocol/lips/`](../lips/) — Ligate Improvement Proposals, including the schema-spec LIPs.
- [`crates/modules/attestation/`](../../../crates/modules/attestation/) — on-chain module that stores `(schema_id, payload_shape_hash)` pairs.
