# Prompt Evaluation Pool (RFC v0.1)

**Status:** design draft. Tracks an open question raised against the [bounty marketplace RFC](bounty-marketplace.md) (chain#385): "the v0 bounty primitive ships for fungible parallel work; how do work-for-hire / deliverable bounties get done without the worker getting stiffed?"

This RFC describes a service-layer pattern that solves the trust gap for non-fungible deliverable bounties WITHOUT any chain code change. The chain's existing `Schema`, `AttestorSet`, `Attestation`, and `Bounty` primitives compose into the pattern naturally; what's missing is the off-chain Evaluation Pool service that sits between worker and buyer.

## The trust gap

The v0 bounty primitive is optimized for fungible parallel work: 10,000 paragraphs each attested as original, 1,000 medical records each attested as anonymised, 100 KYC checks each attested as compliant. Many workers each contribute one of N; the chain pays per accepted attestation; everyone earns.

It does NOT cleanly handle the work-for-hire pattern: "I need a prompt for a Python sentiment-analysis bot, 100 AVOW for the winning submission." The buyer wants the deliverable (the prompt), not just attestations of "a prompt exists that meets criteria." Three things go wrong if you use the naive "buyer = attestor" pattern:

1. **Buyer can steal the deliverable.** Worker reveals the prompt to the buyer for review; buyer uses it without signing the attestation. No on-chain payment. Worker has no recourse.
2. **Competing workers all eat fees.** Five workers submit prompts in parallel; one wins; the other four paid `attestation_fee` for nothing and their work is publicly committed on chain.
3. **Buyer can grief.** In a 1-of-1 (buyer-only) attestor set, the buyer holds all the cards. They review off-chain, refuse to sign, walk away with the IP.

These problems are real. They're why work-for-hire freelance markets exist as escrow-with-arbitration services (Upwork, Fiverr's escrow flow, Bountysource) rather than open marketplaces.

## The pattern: a neutral Evaluation Pool

Insert a third party between worker and buyer: a federated **Evaluation Pool** that runs the worker's prompt against the buyer's test cases, signs an attestation if the prompt satisfies the criteria, and holds the encrypted prompt until on-chain settlement releases it to the buyer.

Four parties:

| Party | What they do |
|-------|--------------|
| **Buyer** | Posts a bounty against the eval-pool schema. Publishes test cases. Funds escrow. Watches for attestations; claims the first one that satisfies. |
| **Worker** | Writes the prompt. Encrypts it to the eval pool. Submits via the eval pool's intake API. |
| **Eval Pool** | M-of-N attestors. Receives encrypted prompts. Decrypts in a controlled environment. Runs against the buyer's test cases. Signs attestations. Holds the encrypted prompt. Releases to buyer post-settlement. |
| **Chain** | The bounty + attestation + payment rails. Already exists; no changes needed. |

## Schema design

One canonical schema for prompt-evaluation bounties:

```
Name:    ligate.prompt-evaluation.v1
Owner:   Ligate Labs (at v0); federated cooperative later
Attestor Set:
  - 5 evaluator keys held by independently-operated nodes
  - Threshold: 3 of 5 (majority required to sign)

Payload spec (off-chain, content-addressed via payload_shape_hash):
  {
    "prompt_hash": "sha256 of the worker's encrypted prompt blob",
    "test_case_set_id": "lid of the bounty's test cases (off-chain blob)",
    "verified_outputs": [
      {
        "input": "...",
        "expected_output_hash": "sha256(...)",
        "actual_output_hash": "sha256(...)",
        "similarity_score": 0.93
      }
    ]
  }

Acceptance criteria (enforced by the pool, NOT by the chain):
  - prompt was independently re-run by ≥ 3 evaluators
  - each evaluator independently reported similarity_score ≥ 0.85 across all test cases
  - no evaluator reported the prompt contains restricted content
  - the bounty's test_case_set_id matches one the buyer registered
```

## End-to-end flow

### Step 1: Buyer posts the bounty

Buyer publishes test cases off-chain (their own server, IPFS, or via the Eval Pool's bounty-intake API for hosting). They get a `test_case_set_id` (a Ligate Identifier that the indexer mirrors).

```rust
PostBounty {
  board_schema_id: <ligate.prompt-evaluation.v1>,
  pool: Amount::new(100_000_000_000),         // 100 AVOW
  per_attestation: Amount::new(100_000_000_000), // single-winner: per = pool
  acceptance: AcceptancePredicate::Any,
  expiry_da_height: <30 days out>,
  dispute_window_blocks: 600,
}
```

Bounty metadata (off-chain blob discoverable via the bounty's id):

```json
{
  "task": "Build a Python bot that scrapes Twitter and summarizes sentiment about a stock ticker",
  "test_case_set_id": "ltc1<hash>",
  "test_cases": [
    {"input": "$NVDA", "expected_output_pattern": "Positive sentiment, N% bullish, sample tweets: [...]"},
    {"input": "$TSLA", "expected_output_pattern": "Mixed sentiment, N% bullish, sample tweets: [...]"}
  ],
  "evaluation_criteria": "Each output must include sentiment label + percentage + ≥ 3 sample tweets",
  "encrypted_to": "<eval pool aggregate pubkey>"
}
```

### Step 2: Worker submits encrypted prompt to the Eval Pool

Worker writes the prompt locally, encrypts it to the pool's aggregate pubkey (5-key threshold encryption, or a wrapping key for each individual evaluator), and POSTs to the pool's intake:

```
POST https://eval.ligate.io/v1/prompts
{
  "bounty_id": "lbt1...",
  "test_case_set_id": "ltc1...",
  "worker_address": "lid1...",        // pays to here on settlement
  "encrypted_prompt": "<base64>",
  "encryption_scheme": "x25519-aes256-gcm"
}
```

### Step 3: Eval Pool runs the prompt

Each of the 5 evaluators independently:
1. Decrypts the prompt with their share of the threshold key
2. Runs the prompt in a sandboxed Python environment (Docker container with no network egress; Firecracker microVM for stronger isolation)
3. Compares output to the bounty's test case expectations
4. Computes a similarity score (cosine similarity on embeddings; or LLM-as-judge rubric scoring)
5. If similarity_score ≥ 0.85 across all test cases AND no policy violations detected: signs the attestation

Threshold: 3 of 5 evaluators must independently arrive at "this prompt satisfies the criteria" for the pool's aggregate signature to issue.

### Step 4: Pool submits the attestation on chain

```rust
SubmitAttestation {
  schema_id: <ligate.prompt-evaluation.v1>,
  payload_hash: SHA256({
    prompt_hash,
    test_case_set_id,
    verified_outputs: [...],  // populated by the eval pool
  }),
  signatures: [3-of-5 evaluator signatures],
  // tx submitter = worker_address (so the worker gets paid)
}
```

The chain accepts the attestation under the canonical `prompt-evaluation.v1` schema; the attestor set's threshold validates the signatures.

### Step 5: Buyer claims the bounty

Buyer watches for attestations against their bounty (indexer's bounty-matching service surfaces this). First acceptable attestation arrives. Buyer submits:

```rust
ClaimBounty {
  bounty_id: <lbt1...>,
  claims: [AttestationClaim { attestation_id: <lat1...> }],
}
```

Chain transfers `pool` AVOW from the module escrow to the attestation's submitter (= the worker's address). Worker is paid. Chain is done.

### Step 6: Prompt reveal post-settlement

The Eval Pool watches the chain for `Bounty/BountyClaimed` events on its registered bounties. When it sees a claim settle for a bounty it has an attestation on:

1. Pool reconstructs the threshold-encrypted prompt blob in memory
2. Pool encrypts the plaintext to the buyer's public key (registered alongside their bounty)
3. Pool delivers via webhook or via signed URL (buyer's choice)

Buyer now has the prompt; worker has been paid; the eval pool earned its fee_routing_bps slice as part of the chain's fee distribution. Everyone got what they wanted.

## Security model

### Threat: buyer steals the prompt without paying

**Defense:** the prompt never leaves the eval pool's controlled environment until on-chain settlement. The buyer can see the attestation (which proves a prompt exists that satisfies their test cases) but not the prompt itself. The reveal happens after the chain's atomic state transition has paid the worker.

### Threat: eval pool colludes with buyer

**Defense:** M-of-N threshold (3 of 5) means majority collusion required. Pool members are independently-operated nodes with reputation at stake (the canonical pool is the showcase for the entire bounty marketplace's credibility). Pool members are publicly identified and their keys are pinned in the schema's attestor set.

**Open question:** does the chain need a dispute mechanism for "the eval pool signed a fraudulent attestation"? The v0 dispute primitive (`DisputeAttestation`) handles this. Anyone (including the worker, including other pool members) can dispute a fraudulent attestation; the bounty board operator resolves.

### Threat: worker submits a prompt that doesn't work but uses adversarial inputs

**Defense:** the eval pool runs the prompt against the BUYER's test cases (not the worker's). The buyer chose the test cases. Worker can't game what they don't pick.

**Mitigation:** the buyer can include adversarial test cases ("does the bot reject malformed input?", "does it handle empty tickers?"). Standard adversarial-testing practice.

### Threat: eval pool gets hacked, leaks prompts to buyers without payment

**Defense:** the threshold encryption means an attacker needs to compromise ≥ 3 of 5 evaluator nodes. TEE / Nitro Enclaves on individual evaluators raise the bar further. Operational security on each pool member is critical; this is the same trust model as a federated PoS validator set.

### Threat: eval pool is centralized at v0

**Acknowledged.** At v0 Ligate Labs operates all 5 evaluator nodes. This is acceptable for bootstrapping (the alternative is no marketplace at all), but the path to federation is the v1 priority. By v1 we want: at least 3 distinct organisations operating the 5 nodes; published transparency reports; an opt-in dispute mechanism for evaluator-misbehaviour.

## Fee economics

The schema's `fee_routing_bps` routes a slice of every attestation fee to the eval pool's treasury, paying for the cost of running the evaluators. Suggested:

- `fee_routing_bps = 1000` (10%) at v0 on the canonical schema
- `fee_routing_addr` points to the eval pool's treasury multisig

The worker gets `pool_per_attestation - (bounty board's routing share)` net of any board fee. The eval pool earns the 10%. The buyer pays once (the bounty pool); the chain handles distribution.

Compute cost estimate (canonical pool, 5 evaluators each running a sandboxed Python eval): roughly $0.01 per evaluation on commodity cloud compute. A 100 AVOW bounty at ~$1/AVOW (testnet pricing, illustrative only) = $100 to the worker, $10 to the pool, covers ~1,000 evaluations per bounty before the pool is loss-making. Healthy ratio.

## v0 to v1 evolution

**v0 (Ligate Labs only):**
- Pool is a single repo + 5 instances running on Ligate Labs infrastructure
- Threshold key managed via Cloud KMS (per the [key management RFC](key-management.md))
- Sandboxing: Docker containers with `--network=none`
- Compute: GCE `e2-standard-2` x 5
- Schema is `ligate.prompt-evaluation.v1` registered by Ligate Labs

**v1 (federated):**
- Pool expands to 5 distinct operating organisations
- Each operator runs their own node, holds their share of the threshold key
- Sandboxing: Firecracker microVMs or TEE-attested execution
- New schema `ligate.prompt-evaluation.v2` if the wire format changes; v1 schema stays live for backwards compat
- Federation governance: pool members vote on adding/removing operators (separate chain governance module #41)

**v2 (decentralised):**
- Permissionless evaluator entry via the chain's staking module ([#50](https://github.com/ligate-io/ligate-chain/issues/50))
- Stake-to-evaluate: lock AVOW against the eval pool's attestor set, earn fee share, get slashed on misbehaviour
- Sybil resistance via stake + on-chain reputation

## Open questions for the next RFC pass

1. **TEE attestation in the attestation payload.** Should the attestation include a TEE attestation report (Nitro Enclave / AMD-SEV / SGX) that proves the prompt was run in a verified-clean environment? Adds value for high-stakes use cases; complexity for v0.
2. **What sandboxing is "enough"?** Docker `--network=none` is the v0 minimum. Firecracker is the v1 target. Per-evaluator gVisor + seccomp? Need an operational-security review.
3. **Prompt-reveal delivery channel.** Webhook? IPFS with buyer's encrypted CID? Signed URL? Sticking with "buyer's choice, registered alongside bounty" but worth tightening.
4. **Eval pool downtime.** If the pool is down for 24h, workers can't submit. Should bounties have a fallback "if no attestation by `T`, refund poster automatically"? Existing `CancelBounty` handles this manually but is gated on poster action.
5. **Worker-side prompt encryption format.** Standardise on libsodium's `crypto_box`? Use chain-level keypairs (workers already have one)? Avoid yet-another-key-management problem for workers.
6. **Buyer-side test case authoring.** A buyer who can't write good test cases will get bad prompts. Should the eval pool offer a "test case review" service? Tracked separately.

## Sub-issues to file after this RFC merges

1. **Build the `ligate-eval-service` repo + binary.** Rust HTTP intake + Python sandbox runner + threshold signature aggregation + chain submission + reveal-on-settlement loop. ~2-3 weeks of focused dev.
2. **Register `ligate.prompt-evaluation.v1` schema on devnet-3+.** Once the service exists, register the canonical schema with the federated attestor set. Update Mneme's discovery surface to show it.
3. **Test case authoring docs at `docs.ligate.io/eval-pool/test-cases.md`.** Best practices for buyers writing test cases. Adversarial input examples. Similarity-score rubrics.
4. **Eval pool transparency dashboard.** Per-evaluator uptime, attestation-issuance rate, dispute outcomes, fee revenue. Probably lives on `eval.ligate.io/transparency` as a status page.

## Acceptance for this RFC

This document landing as `docs/protocol/prompt-evaluation-pool.md` is the acceptance. The actual eval service ships against the sub-issues above. The chain code does not change.

The pattern unblocks the work-for-hire bounty use case without compromising the v0 bounty primitive's clean fungible-work design. Both patterns can run on the same chain side-by-side, distinguished only by which schema a bounty references.
