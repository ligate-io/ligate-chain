---
LIP: <number, assigned at Review>
Title: <short descriptive title>
Author: <Name <email-or-handle>>
Status: Draft
Type: Standards Track
Category: Core
Created: <YYYY-MM-DD>
---

## Abstract

One-paragraph summary of the proposal. A reader should understand what
the LIP does and why from this section alone.

## Motivation

Why does this LIP exist? What problem does it solve, and why does that
problem warrant a protocol change rather than an off-chain or
out-of-band solution?

If the LIP is reactive to a discovered issue (security finding,
performance bottleneck, ecosystem demand), document the trigger
specifically.

## Specification

The actual spec. This section is the LIP's reason for existing.

For Standards Track LIPs, be precise enough that an implementer who
has not been part of the discussion can build a conformant
implementation from this section alone. For schema-category LIPs,
include the full schema definition and example payloads.

Subdivide as the spec grows:

### Wire format / data structures

Borsh layouts, JSON schemas, struct definitions, enum variants. Pin
field types and ordering precisely.

### State

What state does this LIP touch? New maps, modified structs, removed
items. Include the storage path and key derivation.

### Behaviour

State transitions, validation rules, error conditions. The "what
happens when" section.

### Interfaces

REST endpoints, RPC methods, CLI flags, client SDK methods. If the
LIP touches `crates/client-rs`, document the new builder/method
signatures.

## Rationale

Why this design and not an alternative? At minimum:

- What alternative designs were considered?
- What are the trade-offs of the chosen design?
- Where does this LIP draw inspiration from (other chains, existing
  primitives, academic work)?

This is where reviewers focus their attention. A LIP without a
rationale section is hard to review.

## Backwards compatibility

Does this break wire-format compatibility? Existing signatures?
Existing state? Existing client APIs?

If the LIP requires a chain-id bump per
[`upgrades.md`](../upgrades.md#what-counts-as-a-hard-fork), state it
explicitly with the proposed transition (e.g. `ligate-devnet-2` →
`ligate-devnet-2`).

If the LIP is backwards-compatible, explain why (e.g. "appended
`CallMessage` variant, Borsh tags preserved").

## Test cases

For Standards Track LIPs, list the test cases that the implementation
must include. Examples:

- "Round-trip Borsh encode + decode for the new struct layout."
- "Reject `CallMessage::Foo` with `bar > MAX_BAR`."
- "Cross-language test vector: input X produces output Y in both Rust
  and Python implementations."

If reference test vectors live in `crates/modules/.../tests/` or in
external corpus files, link to them here.

## Reference implementation

Link to the implementation PR(s) once they exist. Pre-implementation,
this section may say "not yet implemented" or sketch the rough
shape.

## Security considerations

What attack surfaces does this LIP introduce or modify? Reference
[`docs/protocol/threat-model.md`](../threat-model.md) and document
where this LIP fits in the trust posture.

If the LIP doesn't introduce security-relevant changes, state that
explicitly with reasoning.

## Copyright

Released under the same Apache-2.0 / MIT dual license as the rest
of the chain repo.
