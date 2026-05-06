---
LIP: 1
Title: LIP process
Author: Stefan Stefanović <hello@ligate.io>
Status: Final
Type: Process
Created: 2026-05-06
---

## Abstract

This LIP defines the LIP process itself: what a LIP is, when one is
required, what states a LIP moves through, and who's responsible
for moving it. It is a meta-LIP, modelled on
[EIP-1](https://eips.ethereum.org/EIPS/eip-1) and
[CIP-1](https://cips.celestia.org/cip-1).

## Motivation

Pre-mainnet, Ligate Chain is in a phase where non-trivial protocol
decisions warrant explicit discussion before code lands. A regular
PR is the right tool for "fix this bug" or "add this test"; it's
the wrong tool for "introduce a reputation-weighted consensus
mechanism" or "register the canonical content-provenance schema for
the entire ecosystem". The latter class wants:

- A specification reviewers can cite by number when discussing
  trade-offs.
- A documented decision trail so the rationale survives team
  changes.
- A clear "approved / not approved" signal so implementation work
  starts from a stable spec, not a moving design.

LIPs are the artifact for that class of change. EIPs, CIPs, and BIPs
exist for the same reason on other chains.

## Specification

### What needs a LIP

Captured in [`README.md`](README.md). The short version: anything
that touches the runtime composition, the wire format, consensus,
cross-module invariants, first-party schemas, or the LIP process
itself.

### LIP states

`Draft → Review → Last Call → Final`

with off-ramps to `Withdrawn`, `Rejected`, and `Stagnant`. Lifecycle
detail is in [`README.md`](README.md#lip-lifecycle).

The `Last Call` window is **14 calendar days**. Pre-mainnet this is
short by mature-chain standards (Ethereum uses 14 days, Celestia
uses 14 days, Bitcoin's BIP process is more variable). 14 days is
chosen as the floor that lets external reviewers (existing operators,
prospective design partners, ecosystem contributors who don't follow
the repo daily) surface concerns without delaying the implementation
arm of the team.

If material concerns surface, the LIP returns to `Review`. If no
concerns surface, the LIP advances to `Final`.

### Numbering

Sequential, starting from `LIP-1` (this document). Numbers are
assigned at the `Review` transition, not at `Draft`, to avoid
fragmenting the number space with abandoned drafts.

### File format

Markdown files in `docs/protocol/lips/`, named `lip-N.md`, with
YAML frontmatter per the
[file format section in `README.md`](README.md#file-format).

The body follows [`lip-template.md`](lip-template.md): Abstract,
Motivation, Specification, Rationale, Backwards compatibility,
Test cases, Reference implementation, Security considerations,
Copyright.

### Author and reviewer roles

Authors write and revise LIPs. Maintainers (per `CODEOWNERS`) move
LIPs through the lifecycle and document rejection rationale. The
[dispute resolution path](README.md#dispute-resolution) is the
escalation route when author and maintainers disagree.

Pre-mainnet: maintainer team is small (founder + designated
reviewers); decisions are essentially BDFL-shaped.

Post-mainnet: the
[governance module](https://github.com/ligate-io/ligate-chain/issues/41)
takes over LIP-class decisions for parameter and module changes.

### Implementation

A `Final` LIP is a spec, not a release. The implementation lands as
a separate PR (or set of PRs) that:

- References the LIP number in the commit message (`feat: implement
  reputation-weighted consensus per LIP-2`).
- Adds a CHANGELOG entry citing the LIP and the relevant tracking
  issue.
- Lands in the order the LIP's `Requires:` frontmatter dictates.

## Rationale

### Why a separate process from regular PRs

Three reasons:

1. **Specification stability.** A LIP is a stable spec. An open PR
   is in flux. Implementation work that targets a moving design
   wastes effort. The `Final` state is the signal that the design
   has stopped moving.

2. **Discussion durability.** PR comments are easy to lose track of
   over time. A LIP file in the repo is permanent and citable: "see
   LIP-N section §3.2" is a more durable reference than "see the
   30th comment on PR #872".

3. **External reviewer visibility.** Pre-mainnet, design partners,
   prospective auditors, and ecosystem contributors need a
   well-known surface to find and review the chain's planned
   trajectory. A numbered, linkable, structured document set is
   that surface; an open PR queue is not.

### Why model on EIPs / CIPs / BIPs

Inheriting an established process saves us from rediscovering the
same lifecycle states, the same author-vs-reviewer tensions, and
the same numbering conventions that other chains have already
solved. The cost is some lock-in to a specific shape; the benefit is
that anyone who's contributed to Ethereum / Celestia / Bitcoin
already knows how to read and write a LIP.

### Why 14-day Last Call

Same window as Ethereum and Celestia. Long enough for external
reviewers to weigh in; short enough that the implementation arm
isn't blocked indefinitely on a process step.

If 14 days proves wrong in either direction, this LIP is the place
to amend it.

### Why "Stagnant" auto-transition at 90 days

Without it, the `Draft` and `Review` states become a graveyard of
half-formed ideas that nobody wants to formally close. 90 days is
generous; 60 would also be reasonable. Authors can resurrect a
stagnant LIP by pushing new commits.

## Backwards compatibility

Not applicable. This LIP introduces the LIP process itself; there's
no prior process to be backwards-compatible with.

The repo previously had `docs/protocol/rfcs/` as a placeholder
concept (no files committed, four tracking issues filed: #127,
#183, #184, #185). The LIP process supersedes the RFC concept.
The four RFC tracking issues are now LIP placeholders (LIP-2 through
LIP-5) per the
[numbering table in `README.md`](README.md#numbering).

## Test cases

Not applicable. This is a Process LIP; conformance is measured by
maintainer behaviour, not by code.

## Reference implementation

This LIP and its supporting files (`README.md`, `lip-template.md`)
are themselves the implementation.

## Security considerations

A LIP process can be gamed by an adversary who files many
low-quality drafts to consume maintainer time, or by an author who
withdraws and resubmits the same proposal under different numbers
to circumvent rejection.

Mitigations:

- Maintainers can close `Stagnant` and `Withdrawn` LIPs without
  ceremony.
- Resubmitted-after-rejection LIPs are within scope of maintainer
  judgment to reject again on the same grounds.
- Pre-mainnet, the contributor pool is small enough that
  good-faith assumption holds. Post-mainnet, governance takes over
  and the question becomes one of token-weighted vote rather than
  process-gaming.

The LIP process itself does not handle any cryptographic material,
funds, or production state, so the security surface is essentially
social.

## Copyright

Released under the same Apache-2.0 / MIT dual license as the rest
of the chain repo.
