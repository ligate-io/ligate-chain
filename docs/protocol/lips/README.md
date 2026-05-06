# LIPs: Ligate Improvement Proposals

This directory holds **Ligate Improvement Proposals** (LIPs) — design
documents for non-trivial protocol changes that warrant explicit
discussion, review, and approval before code lands.

The format and process are modelled on
[Ethereum's EIPs](https://eips.ethereum.org/),
[Celestia's CIPs](https://cips.celestia.org/), and
[Bitcoin's BIPs](https://github.com/bitcoin/bips). Same shape; different
scope.

## What needs a LIP

**A LIP is required for**:

- New runtime modules (e.g. `delegation`, `reputation`, `tokens`).
- Wire-format changes to public protocol types (`Schema`, `AttestorSet`,
  `Attestation`, `SignedAttestationPayload`, `CallMessage` shape).
- Consensus-level changes (signature schemes, hashing, ordering rules).
- Cross-module invariants that span multiple modules.
- New first-party schemas that ship with chain genesis (e.g.
  `themisra.proof-of-prompt/v1`, future content-provenance and licensing
  schemas).
- Process changes to LIPs themselves (handled as meta-LIPs).
- Anything that bumps the chain-id ladder per
  [`upgrades.md`](../upgrades.md#what-counts-as-a-hard-fork).

**A LIP is NOT required for**:

- Bugfixes to existing behaviour (regular PR + CHANGELOG entry).
- Internal refactors that preserve behaviour and wire format.
- Test additions, doc fixes, build/CI changes.
- Operational tooling not in the runtime (faucet, indexer, explorer).
- Adding a new DA adapter (already specified by the
  [DA-layers playbook](../da-layers.md#playbook-adding-a-new-da-adapter)).

When in doubt, file an issue describing the change. The maintainers
will tell you whether it needs a LIP.

## LIP lifecycle

```
Draft → Review → Last Call → Final
                          ↘ Withdrawn (by author)
                          ↘ Rejected (by maintainers)
                          ↘ Stagnant (90 days no activity)
```

### Draft

Author opens a PR adding `lip-N.md` to this directory, where `N` is the
next available LIP number. The PR description includes:

- A one-paragraph motivation.
- The problem this LIP solves.
- A proposed solution sketch.

The LIP at this stage is incomplete by design. Drafts are for getting
the *direction* right, not the spec details.

### Review

Once the author thinks the LIP is reasonably complete, they update the
`Status` frontmatter field to `Review` and request review from
maintainers. Discussion happens on the PR. Maintainers may request:

- Clarifications on the specification.
- Alternative designs that were considered.
- Backward compatibility notes.
- Test vectors or reference implementation pointers.

A LIP can bounce between `Draft` and `Review` as the design firms up.

### Last Call

When the LIP is essentially ready and maintainers want broader input
before merging, the status moves to `Last Call`. This is a 14-day
window for non-maintainer feedback (anyone watching the repo, the
`hello@ligate.io` channel, the project Discord/Telegram once those
exist). If material concerns surface, the LIP returns to `Review`. If
no concerns surface in 14 days, the LIP advances to `Final`.

### Final

The LIP is locked. Frontmatter `Status` flips to `Final`, the PR
merges, and any future modification requires a new LIP that
supersedes this one.

A LIP being `Final` does not mean it ships immediately. It means the
spec is approved. The implementation lands as a separate PR (or set
of PRs) that references the LIP number in the commit message and
CHANGELOG entry.

### Rejected

Maintainers may reject a LIP at any stage with a documented reason
(scope conflict, security concern, design alternative is strictly
better, etc.). Frontmatter `Status` flips to `Rejected`, the PR is
closed, and the LIP file remains in the repo as a record of the
discussion.

### Withdrawn

The author may withdraw their own LIP at any pre-`Final` stage.
Frontmatter `Status` flips to `Withdrawn`. Like `Rejected`, the file
remains in the repo.

### Stagnant

A LIP in `Draft` or `Review` with no activity for 90 days
auto-transitions to `Stagnant`. The author can revive it by pushing
new commits or comments; otherwise it's effectively `Withdrawn` with
the option to resurrect.

## Author and reviewer responsibilities

**Authors**:

- Write the LIP. Be clear about what's being proposed and why.
- Engage with reviewer feedback. Substantive feedback earns a
  substantive response, not just acknowledgement.
- Update the LIP as discussion progresses.
- File the implementation PR(s) once the LIP is `Final` (or coordinate
  with whoever does the implementation).

**Maintainers** (currently `@ligate-io/maintainers`,
`@ligate-io/protocol-reviewers` per `CODEOWNERS`):

- Review LIPs in good faith. Don't gatekeep based on style; do
  gatekeep on substance.
- Move LIPs through the lifecycle promptly. A `Draft` shouldn't sit
  for weeks because no maintainer looked at it.
- Document rejection rationale clearly. The LIP file is the record;
  future authors should understand why their predecessor was rejected.

## Dispute resolution

If author and maintainers disagree on a LIP's direction:

1. Surface the disagreement in the PR. Quote the specific concern.
2. If unresolved, the maintainer team makes a final call documented
   in the PR comments.
3. The author's options are: revise to address the concern, withdraw,
   or accept rejection.

Pre-mainnet, the maintainer team is small (the founder plus designated
reviewers from `CODEOWNERS`); decisions are essentially BDFL-shaped.
Post-mainnet, the
[governance module](https://github.com/ligate-io/ligate-chain/issues/41)
takes over LIP-class decisions for parameter and module changes.

## Numbering

LIPs are numbered sequentially starting from `LIP-1`. The number is
assigned when the LIP first enters `Review` state, not at `Draft`.

| LIP | Title | Status |
|---|---|---|
| [LIP-1](lip-1.md) | LIP process | Final |
| LIP-2 | _(reserved for PoUA, see [#127](https://github.com/ligate-io/ligate-chain/issues/127))_ | _(not yet drafted)_ |
| LIP-3 | _(reserved for receipt embedding spec, see [#183](https://github.com/ligate-io/ligate-chain/issues/183))_ | _(not yet drafted)_ |
| LIP-4 | _(reserved for `themisra.content-provenance/v1`, see [#184](https://github.com/ligate-io/ligate-chain/issues/184))_ | _(not yet drafted)_ |
| LIP-5 | _(reserved for `themisra.prompt-licensing/v1` + `themisra.content-licensing/v1`, see [#185](https://github.com/ligate-io/ligate-chain/issues/185))_ | _(not yet drafted)_ |

Numbers are reserved when a tracking issue is filed; the LIP file
itself is added when an author starts the draft.

## File format

All LIPs are markdown files in this directory, named `lip-N.md`. Each
file starts with frontmatter:

```yaml
---
LIP: N
Title: One-line title
Author: Name <email-or-handle>
Status: Draft | Review | Last Call | Final | Rejected | Withdrawn | Stagnant
Type: Standards Track | Process | Informational
Category: Core | Networking | Interface | Schema  # only for Standards Track
Created: YYYY-MM-DD
Updated: YYYY-MM-DD  # optional, last meaningful update
Requires: lip-X, lip-Y  # optional, dependencies
Replaces: lip-Z  # optional, if this LIP supersedes another
Discussion: <URL>  # optional, link to PR or external discussion
---
```

The body follows the structure in
[`lip-template.md`](lip-template.md).

### Type and Category

- **Standards Track**: changes that affect protocol implementations.
  Sub-categories:
    - `Core`: consensus, runtime, wire format, signature schemes
    - `Networking`: peer-to-peer, sequencer routing, DA layer
      interaction
    - `Interface`: REST API, client SDK contracts, CLI flags
    - `Schema`: first-party schemas (themisra.proof-of-prompt,
      content-provenance, licensing, etc.)
- **Process**: LIPs about the LIP process itself, governance,
  maintainer team structure (e.g. LIP-1)
- **Informational**: LIPs that document existing behaviour, design
  rationale, or operator practices without specifying a change

## Spinning out to a separate repo

The LIP catalog lives in this directory until it grows enough to
warrant its own repo. The trigger is **≥3 LIPs in `Final` state** at
which point we'll split to `ligate-io/lips` (sibling tracking issue
filed when that happens), set up an mdBook renderer, and redirect
`lips.ligate.io` at it. Until then, markdown-in-repo is enough.

## Related

- [`docs/protocol/upgrades.md`](../upgrades.md) — soft / hard fork
  policy, chain-id ladder. Hard-fork-class LIPs are gated by this.
- [`docs/protocol/da-layers.md`](../da-layers.md) — DA adapter
  playbook (does not require a LIP).
- [`CONTRIBUTING.md`](../../../CONTRIBUTING.md) — general contributor
  guide. LIPs follow the same commit / PR conventions.
- [`CODEOWNERS`](../../../.github/CODEOWNERS) — review routing for
  protocol-track LIPs.
- Tracking issue: [#211](https://github.com/ligate-io/ligate-chain/issues/211).
