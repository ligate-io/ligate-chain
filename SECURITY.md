# Security policy

Ligate Chain is a sovereign rollup that handles attestation receipts, $LGT
balances, and (via the curated module set) tokens, NFTs, and identity records.
Security issues in this repository can affect chain liveness, the integrity
of attestations, or user funds once mainnet launches. We take them
seriously and want to make it easy for researchers to report them.

This policy applies to **`ligate-io/ligate-chain`**. Product applications
(Themisra, Iris, Mneme) live in separate repositories; each carries its own
SECURITY.md that takes precedence for issues scoped to that product.

For the current attacker model, the v0 protections in place, and what is
deliberately not modeled yet, see
[`docs/protocol/threat-model.md`](docs/protocol/threat-model.md).

## Where to report

**Use GitHub Private Security Advisories — preferred channel.**

Open a private advisory at
<https://github.com/ligate-io/ligate-chain/security/advisories/new>.
This encrypts the report to repository admins and gives us a private
discussion thread, draft CVE assignment, and coordinated disclosure
tooling out of the box.

**Email — fallback if you can't use GitHub.**

Send to <hello@ligate.io> with a subject line starting `[SECURITY]`. We
triage all `hello@` traffic; the prefix routes it to the maintainer queue
within one business day.

A dedicated `security@ligate.io` PGP-keyed inbox is on the roadmap and
will replace `hello@` for security reports once mainnet is closer. Until
then, GitHub Private Security Advisories is the encrypted channel.

**Please do not open public GitHub issues or pull requests for
suspected vulnerabilities.** A public issue makes the bug exploitable
the moment it's filed.

## What to include

A useful report has:

- A clear description of the vulnerability and the impact you observed.
- The chain commit hash (`git rev-parse HEAD` from the affected build) or
  the released version, if applicable.
- Reproduction steps. A minimal repro on the local devnet
  (`cargo run --bin ligate-node`) is ideal; we can mirror it on our side.
- The class of impact: chain halt, invalid state transition, signature
  bypass, fee routing error, etc.
- Optionally: a proposed fix or mitigation. Not required for triage.

## What we will do

Within **2 business days** of receipt, we will:

1. Acknowledge the report.
2. Assign one maintainer as the point of contact.
3. Confirm whether we can reproduce the issue.

For confirmed vulnerabilities, we will keep you informed through the
investigation, agree a coordinated disclosure timeline, and credit you
in the eventual public advisory unless you prefer to remain anonymous.

We aim for a fix within **30 days** for critical issues affecting chain
liveness, signature validation, or balance accounting. Less critical
issues (UX, peripheral tools, non-exploitable error paths) follow normal
release cadence.

## Scope

In scope:

- The `ligate-node` binary (`crates/rollup`) and its DA adapters.
- The state-transition function (`crates/stf`, `crates/stf-declaration`).
- The `attestation` module (`crates/modules/attestation`).
- The Risc0 prover (`crates/rollup/provers/risc0`) and the Celestia
  guest (`crates/rollup/provers/risc0/guest-celestia`).
- The Rust client SDK (`crates/client-rs`).
- The CI workflows under `.github/workflows/`.
- Devnet bootstrap configs in `devnet/` (genesis JSONs, rollup/celestia
  TOMLs).
- Any documentation in `docs/protocol/` whose claims a real chain user
  would rely on.

Out of scope here (report to the relevant project's policy):

- Marketing site (`ligate.io`) and docs site (`docs.ligate.io`).
- Themisra (chat app, attestor quorum service, schema crate).
- Iris (MCP server, USD-billed relayer).
- Kleidon (Passify, SkinsVault, TokenForge, MintMarket).
- Mneme browser wallet.
- The Sovereign SDK, Risc0, Celestia, and other upstream dependencies —
  those go to the upstream project's security contact. We will help
  coordinate if a downstream issue lands here first.

## Bug bounty

There is **no formal bug bounty programme yet**. Pre-public-devnet, the
chain holds testnet $LGT only and there are no real funds at risk;
running a paid programme on a pre-launch system attracts noise more than
signal. We plan to introduce a programme alongside the v1 mainnet
launch, scaled to the protocol's TVL at the time.

We may issue ad-hoc rewards for high-impact reports between now and
then. Reach out and we'll talk.

## Safe harbour

Good-faith research on Ligate Chain that follows this policy is
welcome. We will not pursue legal action against researchers who:

- Report vulnerabilities through the channels above rather than
  exploiting them publicly.
- Make a reasonable effort to avoid privacy violations, data
  destruction, or service disruption while testing.
- Stay within the scope above and don't pivot to systems we don't
  operate.

If a vulnerability requires accessing data you don't own, **stop and
report what you have**. We'd rather have a partial report and a quick
follow-up than a full chain of exploitation.

## Coordinated disclosure

We default to **90-day coordinated disclosure** from the date of report
acknowledgement, with the option to extend by mutual agreement if a fix
needs more runway (e.g., upstream Sovereign SDK changes are required).
After the embargo, we publish a security advisory with credit to the
reporter, a description of the issue, the affected versions, and the
fix.

If you find a vulnerability in code that's been forked or vendored from
us, please loop us in alongside the downstream maintainer; cross-repo
issues are common at the rollup-framework layer.

---

Thank you for helping keep Ligate Chain safe.
