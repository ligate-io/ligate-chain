# Audits

This directory holds external security audit reports and the
remediation tracking that follows from them. Empty today, by design:
no audit has been booked yet, and audits land closer to mainnet
when the protocol surface is stable enough to hold up under
adversarial review.

This file documents the convention so the first audit lands in the
right shape on day one.

## When audits happen

Pre-mainnet, in a phased pattern aligned with the
[chain-id ladder](../protocol/upgrades.md#what-counts-as-a-hard-fork):

- **Pre-`ligate-testnet-1`**: scoped audit of the attestation
  module, the genesis tooling, the runtime composition, and the
  Borsh wire format. Smaller scope, focused on the core protocol
  surface that locks at mainnet genesis.
- **Pre-`ligate-1` mainnet**: full-protocol audit including any
  primitive modules that landed during the testnet phase
  (delegation, per-schema fees, reputation/PoUA, etc.). Larger
  scope, multiple firms in sequence if the budget allows.

Post-mainnet audits trigger off the
[upgrade module ceremony](../protocol/upgrades.md#post-mainnet-governance--upgrade-modules)
when a new primitive module lands. Cosmos chains audit roughly
once per major upgrade; we'll match that cadence.

This directory does not gate on a specific calendar date. The
trigger is "the surface is stable enough to be worth auditing", not
"two months before mainnet".

## Naming convention

Each audit lands as a paired set of files in this directory:

```
docs/audits/
  <YYYY-MM>-<auditor>-<scope>.pdf
  <YYYY-MM>-<auditor>-<scope>.md
```

Where:

- `<YYYY-MM>` is the audit completion month, not the kickoff month
  (so `2026-09` means "delivered in September 2026")
- `<auditor>` is a short slug for the firm: `informal`,
  `trail-of-bits`, `oak`, `zellic`, `cantina`, etc. Lowercase,
  hyphen-separated, no spaces
- `<scope>` is a short slug describing what was audited:
  `attestation-module`, `full-protocol`, `genesis-tooling`,
  `runtime-composition`. Same casing rules

Example entries (none of these exist yet):

```
docs/audits/
  2026-09-informal-attestation-module.pdf
  2026-09-informal-attestation-module.md
  2027-02-trail-of-bits-full-protocol.pdf
  2027-02-trail-of-bits-full-protocol.md
```

The PDF is the original auditor deliverable, checked in verbatim.
The markdown sidecar is ours: it tracks the audit's findings and
the remediation status of each.

## Markdown sidecar shape

Each `.md` file follows this skeleton:

```markdown
# <Auditor> audit of <scope> (<YYYY-MM>)

**Auditor**: <link to firm>
**Scope**: <one paragraph: what they reviewed, what they didn't>
**Commit audited**: <git SHA on `main` at audit kickoff>
**Final report**: [<filename>.pdf](<filename>.pdf)
**Disclosure date**: <YYYY-MM-DD>

## Findings summary

| ID | Severity | Status | Description | Remediation |
|---|---|---|---|---|
| F-01 | Critical | Fixed | One-line description | PR #N, CHANGELOG entry |
| F-02 | High | Fixed | ... | PR #N |
| F-03 | Medium | Acknowledged | ... | Documented as accepted risk in <link> |
| F-04 | Low | Won't fix | ... | Rationale: ... |
| F-05 | Informational | Fixed | ... | PR #N |

## Notes

Any auditor recommendations that don't fit the findings table
(architectural observations, future-work suggestions, scope
limitations the auditor flagged). Inline prose is fine.
```

The findings table cross-links every `Fixed` row to the PR that
landed the fix and the corresponding `CHANGELOG.md` entry. That way
the audit-to-CHANGELOG-to-source-of-truth chain stays auditable
itself.

`Severity` levels match the
[CVSS-like scale](https://docs.cvedetails.com/) most audit firms
use: Critical / High / Medium / Low / Informational. We don't
adjust the auditor's severity assignment downward without a
documented reason.

`Status` is one of:

- `Fixed`: PR landed, CHANGELOG entry filed, fix verified by the
  test suite or by manual review noted in the sidecar
- `Acknowledged`: finding is real, accepted as risk for now,
  rationale documented (e.g., "Medium severity because affects
  a code path that requires sequencer compromise; addressed by
  sequencer decentralisation in v1+")
- `Won't fix`: finding is rejected with rationale (e.g., "out of
  scope for the audited surface; the assumption the finding
  contradicts is documented in [`threat-model.md`]")

## Cross-references

Every audit finding's remediation should leave a trail in the
`CHANGELOG.md` so users tracking releases see the security
context. Format: a `### Security` group entry under the relevant
`[Unreleased]` or version block.

Example CHANGELOG entry:

```
### Security

- Tighten `Schema::derive_id` collision domain per the Informal
  Systems audit (F-02, High). The previous derivation rule
  allowed adversary-controlled `name` to collide with an existing
  schema if the attacker chose `name` to hash-extend. Fixed by
  domain-separating the SHA-256 input. Closes [#312]. Audit
  reference: [`docs/audits/2026-09-informal-attestation-module.md`].
```

## What this directory does not contain

- Internal-only security reviews (those land in private repos /
  Notion as part of routine ops)
- Bug bounty disclosures (those run through a separate
  disclosure-policy track, tracked separately when bug-bounty
  ships)
- Self-audits / scripted vulnerability scans (the `cargo audit`
  output, the fuzz corpus, the `check-da-isolation.sh` guard).
  Those sit closer to the code in `audit.toml`,
  `crates/modules/attestation/fuzz/`, and `scripts/` respectively
- Compliance attestations (SOC 2, etc.). Out of scope for an
  open-source rollup at this stage

## Tracking issue

[#208](https://github.com/ligate-io/ligate-chain/issues/208).
