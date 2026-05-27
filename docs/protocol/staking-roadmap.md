# $AVOW staking roadmap

**Status:** narrative doc; complements the [pre-testnet economics RFC](pre-testnet-economics.md) (chain#523) and the [staking module umbrella](https://github.com/ligate-io/ligate-chain/issues/50)
**Audience:** $AVOW holders without operator infrastructure, asking "where does my yield come from and when?"
**Tracks:** [chain#526](https://github.com/ligate-io/ligate-chain/issues/526)

---

## TL;DR

- **Testnet:** holders without operator infrastructure have no yield surface. This is honest, not hidden.
- **v1 (post-#50):** stake-to-attest delegation goes live. Holders lock $AVOW against an attestor set, earn a slice of every attestation fee paid through that set.
- **v1+:** bond pools, insurance pool, schema bonding stack on top of the same staking primitive.

The order matters: we do not promise yield before the module that pays it exists.

---

## 1. Where $AVOW value comes from today (testnet)

Two live revenue surfaces, neither of which routes to holders:

### Operator share

The federated sequencer (Ligate Labs only at testnet) and schema-curated attestor federations earn the operator share of every transaction. `Schema.fee_routing_bps` already routes a configurable slice to the schema author at attestation time; the rest of the attestation fee stays with the attestor pool. See [pre-testnet economics RFC](pre-testnet-economics.md) item C.

The operator path goes permissionless via [#50](https://github.com/ligate-io/ligate-chain/issues/50) (staking module) plus [#82](https://github.com/ligate-io/ligate-chain/issues/82) (leader rotation). Until then, "running infrastructure" is a Ligate Labs activity, not a community one.

### Labor share (buyer-funded)

Bounty buyers post bounties in $AVOW; attestors earning the bounty receive $AVOW. This is the testnet pitch and the only direct on-chain inflow for non-operators. See the [bounty marketplace RFC](bounty-marketplace.md) (chain#385) plus the anchor-buyer LOI tracked in [#520](https://github.com/ligate-io/ligate-chain/issues/520).

### What this means for holders

If you hold $AVOW at testnet but don't run an attestor or earn bounties, your position is purely speculative on protocol adoption. There is no yield drip from the protocol to your wallet. Hiding this is the kind of "data union graveyard" trap the pre-testnet RFC's rule 3 explicitly rules out.

---

## 2. Where yield surfaces with staking (v1)

[#50](https://github.com/ligate-io/ligate-chain/issues/50) ships a `staking` module that introduces the first holder-side yield surface:

- **Stake-to-attest delegation.** Lock $AVOW against an `AttestorSetId`. The pool earns a configurable share (`stake_reward_bps`, default 30%) of attestation fees routed through any schema bound to that set, pro-rata to your stake.
- **Slashing.** When the [disputes module](https://github.com/ligate-io/ligate-chain/issues/51) convicts an attestor in the set, your stake takes a proportional haircut. This is the "21 signers with economic skin" promise.
- **Cooldown.** Unbonding has a cooldown period (default ~14 days at devnet block cadence) before withdrawal. Standard Cosmos-pattern stake-then-cooldown shape.

The accumulator math, fee-flow integration, and slashing hook are all designed in #50. Once the module ships:

1. Attestation fee splits become: builder share, staker share, treasury share.
2. Stakers claim from the pool accumulator; payout is O(1) per claim.
3. The "how do I run a node and earn?" community question gets a non-operator answer: you don't have to run a node, you stake against operators you trust.

---

## 3. Where yield expands (v1+)

Three more pools sit on top of the same staking primitive once #50 lands:

- **Insurance pool stakers.** Real demand surfaces post-mainnet when high-stakes attestations (medical, legal, financial) attract consumer-side claims. Stakers in the insurance pool absorb claim payouts in exchange for a slice of premium revenue. Deferred to v1+; see pre-testnet RFC decision 3.
- **Refundable attestation bond pool stakers.** Attestors post a refundable bond at attestation time; failed disputes return the bond plus a fee. Stakers in the bond pool front the bond capital and earn fees. Deferred to v1+; see pre-testnet RFC decision 6.
- **Verified-by-stake schema bonding.** Schema authors lock $AVOW against their schema to signal long-term commitment; downstream consumers see "bonded" status. Stakers can back schemas they expect to succeed. Deferred to v1+ once the disputes module hardens.

All three depend on #50 plus #51 (disputes). None are testnet-blocking.

---

## 4. Timeline

| Milestone | Holder yield surface | Status |
|-----------|----------------------|--------|
| Testnet | None (operator + labor only) | Live |
| v1 (post-#50 + #51) | Stake-to-attest delegation pool | Specced in #50; multi-month implementation |
| v1+ (post-v1 stabilises) | Bond pool, insurance pool | Deferred; not blocking testnet |
| v2+ | Liquid staking position tokens (`lst1` prefix reserved) | Future-work, not committed |

The honest read: holders earn nothing from the protocol at testnet. That is the cost of not running on inflation-funded reward loops (pre-testnet RFC rule 1). The bet is that buyer-funded protocol revenue, routed through the staking module from v1 onward, is more durable than emission farming, and worth the wait.

---

## 5. What this doc does NOT promise

- A specific testnet-to-v1 calendar. The chain has shipped 14 minor releases between v0.2 and v0.3.1 over six weeks; #50 + #51 are estimated as multi-month module work. Calendars get committed once #50 is in flight, not before.
- A yield APR. Returns depend on attestation volume, fee parameters, and competition among stakers. The protocol exposes the mechanism; the market sets the rate.
- Inflation. There is no plan for inflationary staking rewards. Yield is paid from attestation fees, which are paid by attestation buyers. The cap on total returns is total fee revenue, not minting.
- Cross-chain yield. The $AVOW token lives on Ligate Chain. Bridging is post-mainnet (pre-testnet RFC decision 4); cross-chain staking surfaces are not on the roadmap.

---

## 6. Pointers

- [#50](https://github.com/ligate-io/ligate-chain/issues/50): staking module umbrella (the primitive)
- [#51](https://github.com/ligate-io/ligate-chain/issues/51): disputes module (the slashing hook)
- [#82](https://github.com/ligate-io/ligate-chain/issues/82): sequencer leader rotation (the operator side of the same v1 story)
- [#258](https://github.com/ligate-io/ligate-chain/issues/258): long-form Token Model RFC (validator economics, attestor delegation, fee markets, retail UX)
- [pre-testnet-economics.md](pre-testnet-economics.md): the cut-line doc that says which primitives ship at testnet vs spec vs defer
- [bounty-marketplace.md](bounty-marketplace.md): the labor-vector primitive that ships at testnet

---

## Acceptance for this doc

This doc landing as `docs/protocol/staking-roadmap.md` is the acceptance for [chain#526](https://github.com/ligate-io/ligate-chain/issues/526). It surfaces as one of the four acceptance items for [chain#383](https://github.com/ligate-io/ligate-chain/issues/383), and gets quoted in the testnet launch post so holders can read the honest version of the capital-vector story before they buy.
