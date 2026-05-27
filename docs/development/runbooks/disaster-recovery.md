# Runbook, Disaster Recovery

**Status:** v0, the "after" half of [`incident-response.md`](./incident-response.md). Procedures for the four failure modes that escape routine ops.
**Tracks:** [chain#513](https://github.com/ligate-io/ligate-chain/issues/513), child of [chain#360](https://github.com/ligate-io/ligate-chain/issues/360)

A written DR runbook that has never been exercised is theatre. Each scenario below has a "rehearsal checklist" indicating whether we have actually run the procedure end-to-end. Anything marked unrehearsed must be rehearsed on `ligate-localnet` (or a throwaway VM) before mainnet.

---

## Scenarios at a glance

| Scenario | Trigger | RTO (target) | Rehearsed? |
|----------|---------|--------------|------------|
| RocksDB corruption | Chain detects invariant violation; node refuses to start with corruption error | <1h | ✅ via [`backup-restore.md`](./backup-restore.md) drill |
| Total VM loss | Sequencer VM unrecoverable (cloud-side failure, ransomware, instance deletion) | <4h | ❌ pre-mainnet rehearsal required |
| Celestia DA outage >N hours | Celestia mocha-4 down past our patience threshold | depends on duration | ❌ pre-mainnet rehearsal required |
| Sequencer / operator / DA key compromise | See [`incident-response.md`](./incident-response.md) Scenario 4 | <30min contain, <8h rotate | ❌ pre-mainnet rehearsal required |

RTO = Recovery Time Objective. The time from "we declare disaster" to "chain healthy again."

---

## Scenario A, RocksDB corruption

The most common DR scenario, the chain detects state-machine invariant violations (an attestation that fails verification, a transition that contradicts on-chain state). The chain refuses to advance and logs a clear error.

### Detection

- `ligate-node` exits with `state corruption detected at slot X`
- Restart loop with the same error
- `journalctl -u ligate-node` shows RocksDB read failure or assertion failure

### Procedure

1. **Confirm the corruption is in state, not in the binary.** Try a previous binary version via [`chain-upgrade.md`](./chain-upgrade.md) auto-rollback. If the previous binary also fails on the same RocksDB, the corruption is on the state side.

2. **Stop the node.**
   ```sh
   sudo systemctl stop ligate-node
   ```

3. **Snapshot the corrupted state for forensics** (NOT for restoration, the forensics matter for the post-mortem). Tarball `/var/lib/ligate/rocksdb` to a different path before overwriting.

4. **Restore from the latest known-good snapshot.** Follow [`backup-restore.md`](./backup-restore.md) §"Restore procedure." This brings the chain back to the slot of the snapshot.

5. **Replay forward from the snapshot.** The chain catches up to current Celestia tip automatically once the node restarts; the canonical batch stream lives on Celestia. Watch `/v1/ledger/slots/latest` advance.

6. **Verify integrity.** Run the rollup-info + slot-advance + light-node-sync checks from `incident-response.md` Scenario 1 "declare resolved when" list.

### What we cannot do

If all snapshots are corrupt (the corruption predates the oldest snapshot), the only remaining option is **rebuild from DA from genesis**. This is hours of replay. The state-snapshot-publishing follow-up at [#273](https://github.com/ligate-io/ligate-chain/issues/273) makes this faster post-ship.

### Rehearsal

The quarterly restore drill in [`backup-restore.md`](./backup-restore.md) covers steps 4-6. The "snapshot the corrupted state" step (3) is the only step not in the drill; add a synthetic dirty-RocksDB step to the next drill iteration to cover it.

---

## Scenario B, total VM loss

The sequencer VM is unrecoverable. Possible causes: cloud-provider hardware failure, accidental `gcloud compute instances delete`, ransomware on the VM, region-wide GCP outage.

The chain's persistent state is on a separate persistent disk (`/dev/sdb`, RocksDB on `/var/lib/ligate/rocksdb`), so disk loss is a different story from VM loss. This scenario assumes the disk is also gone or unrecoverable.

### Procedure (rebuild from GCS snapshot + Celestia DA)

1. **Provision a new VM** via [`public-devnet-deploy.md`](../public-devnet-deploy.md). Same machine type (`e2-standard-2` for devnet, larger for mainnet), same OS, same disk size.

2. **Cloud-init bootstrap** installs the `ligate-node` binary, Caddy, log-shipping agent, etc. Per [`public-devnet-deploy.md`](../public-devnet-deploy.md).

3. **Restore RocksDB from the latest GCS snapshot.** [`backup-restore.md`](./backup-restore.md) procedure. Determines the slot the chain restarts from.

4. **Re-attach DNS + Cloudflare.** Update Cloudflare A record for `rpc.ligate.io` to the new VM's external IP. Confirm CF cache purged (`cloudflare-cli purge`). Status page update for the brief downtime.

5. **Restore the operator key + DA signer** from GCP Secret Manager (or KMS once #514 ships). The keys live outside the VM exactly so this step is "fetch and configure," not "regenerate and re-genesis."

6. **Restart `ligate-node`.** Catches up from snapshot slot to current Celestia tip.

7. **Verify** via the standard healthcheck panel + a synthetic transaction (drip from the faucet, check it lands).

### Common failure modes during rebuild

| Failure | Mitigation |
|---------|------------|
| Cloud-init fails (network, package install) | Inspect `/var/log/cloud-init-output.log`, run failed steps manually, file infra bug |
| Latest GCS snapshot is corrupt | Fall back to previous snapshot (drift by snapshot interval) |
| All GCS snapshots are corrupt | Replay from DA, see Scenario A "What we cannot do" |
| Cloudflare propagation slow | Set CF TTL low (60s) on the A record so failover is bounded; document in `public-devnet-deploy.md` |

### Rehearsal

**Not rehearsed.** Pre-mainnet must-have: spin up a parallel test VM, follow steps 1-7 end-to-end, time the result. Target RTO 4h; if the rehearsal takes longer, automate the bottleneck. Filed as a follow-up to #513.

---

## Scenario C, Celestia DA outage extending beyond N hours

Celestia mocha-4 testnet is down. We can't post blobs. The chain stalls.

This is the scenario where N matters most. Short outages (<1h) are routine and need no DR; we wait. Long outages (>4h) start hurting partners. Indefinite outages force a hard choice.

### Detection

- `ligate-node` logs `failed to submit blob to celestia` repeatedly
- Celestia community channels confirm a network-wide issue
- `celestia-appd query block latest` from a separate Celestia node times out or returns very stale heights

### Step 1, wait (the first N hours)

Set N based on partner tolerance. Devnet: 4 hours (waited an hour once and recovered cleanly; longer would have triggered a comms cycle). Mainnet: TBD, depends on the SLA we commit to in the testnet launch post.

During the wait:
- Status page update: "DA layer is degraded; chain block production paused. Monitoring."
- No X post yet; Celestia community channels are the source of truth for resolution time.
- Do NOT make any chain-side changes. Resumption is a Celestia-side event; preserve our state to resume cleanly.

### Step 2, if the outage exceeds N (the hard call)

Three options, in escalating commitment:

| Option | Trigger | Reversibility |
|--------|---------|---------------|
| **Wait longer** | Celestia ETA shows resolution in <24h | Always reversible |
| **Fork to MockDa for a known operator-only window** | Partner pressure + Celestia ETA >48h | Reversible only by replaying-and-discarding the MockDa segment when Celestia recovers; messy |
| **Switch DA provider mid-chain** | Celestia announces multi-week outage | Effectively re-genesis; chain id bumps |

We do not have a MockDa fork procedure in v0. Reaching this step on mainnet is a chain-level emergency; for devnet it's an experiment. The hard call is the IC's call (see [`incident-response.md`](./incident-response.md) Scenario 5 step 1 for the framing).

### Step 3, resume after Celestia recovers

If we waited: `ligate-node` automatically resumes blob submission. No DR action needed beyond a status page update.

If we forked: rebuild from Celestia, see [`chain-id-bump.md`](./chain-id-bump.md). Coordinate with partners; the MockDa-segment txs are not on Celestia and need re-submission (if they're worth preserving) or are forfeit.

### Rehearsal

**Not rehearsed.** Short-outage waits have happened; long-outage MockDa-fork has not. Hard to rehearse without an artificial Celestia outage; the rehearsal here is a tabletop exercise (walk through the options with a 48h hypothetical) rather than a live drill. Tabletop noted as a follow-up to #513.

---

## Scenario D, sequencer / operator / DA key compromise

See [`incident-response.md`](./incident-response.md) Scenario 4 for the immediate-containment procedure. This section covers the DR-side recovery after containment.

### The chain id bumps

Operator + sequencer rotation are coupled to chain-id-bump (the new pubkey becomes part of the chain hash). See [`operator-key-rotation.md`](./operator-key-rotation.md) Procedure B + [`chain-id-bump.md`](./chain-id-bump.md).

DA key rotation does NOT bump chain id (it's a DA-layer key, not a chain-state key). See [`da-signer-rotation.md`](./da-signer-rotation.md) Procedure B.

### Recovery steps post-rotation

1. **Verify the new keys are signing correctly.** Watch a few batches land with the new chain-side signing key and a few DA blobs land from the new wallet.
2. **Indexer Postgres TRUNCATE.** Stale rows from the pre-rotation chain id will not merge into the new chain id's data. Procedure in [`backup-restore.md`](./backup-restore.md) §"Indexer state reset across chain id bump."
3. **Downstream service config update.** Every env var or config file that references the old operator address or chain id needs updating. Inventory in [`operator-key-rotation.md`](./operator-key-rotation.md) Procedure A step 4.
4. **Public post-mortem within 7 days.** Format in [`incident-response.md`](./incident-response.md) Scenario 4 step 4.

### Rehearsal

**Not rehearsed.** Key rotation has not been run end-to-end against a partner-facing chain. Filed as a follow-up to #514, the operator-key-rotation runbook explicitly requires a localnet rehearsal before any mainnet rotation event.

---

## Recovery-time accounting

The Recovery Time Objectives in the table at the top are targets, not commitments. Real recovery time depends on which scenario, how far the corruption propagated, how much state needs replay, how cold the rebuild path is.

For mainnet, we publish committed RTOs only after each scenario has been rehearsed at least twice with consistent timing. Pre-mainnet, the RTOs above are targets to aim at while rehearsing.

---

## Post-DR checklist

After every disaster-recovery event:

- [ ] Chain producing blocks at expected cadence
- [ ] All partner integrations confirmed working (drip endpoint, attestation submission, indexer reads)
- [ ] Status page reflects "resolved"
- [ ] Cloud Audit Log evidence preserved in the retention sink (see [`gcp-audit-logs.md`](./gcp-audit-logs.md))
- [ ] Post-mortem started; published within timeline (72h for SEV-1, 7d for SEV-2)
- [ ] Runbook updates filed as PRs for procedure gaps discovered during the event
- [ ] Next quarterly drill scheduled (so the muscle memory does not atrophy)

---

## Cross-references

- [`incident-response.md`](./incident-response.md): the "in the moment" playbook this runbook continues from
- [`backup-restore.md`](./backup-restore.md): RocksDB snapshot + restore procedure (used by Scenarios A and B)
- [`chain-upgrade.md`](./chain-upgrade.md): in-place binary swap with auto-rollback (used by Scenario A step 1)
- [`chain-id-bump.md`](./chain-id-bump.md): genesis + chain id ladder bump (used by Scenarios C and D)
- [`operator-key-rotation.md`](./operator-key-rotation.md): operator wallet key rotation (used by Scenario D)
- [`sequencer-key-rotation.md`](./sequencer-key-rotation.md): chain-side ed25519 signing key rotation (used by Scenario D)
- [`da-signer-rotation.md`](./da-signer-rotation.md): Celestia DA wallet rotation (used by Scenario D)
- [`gcp-audit-logs.md`](./gcp-audit-logs.md): forensic trail (used by post-DR evidence preservation)
- [`multi-sequencer-failover.md`](./multi-sequencer-failover.md): cluster failover (post-#82 future state)
- [`public-devnet-deploy.md`](../public-devnet-deploy.md): full deploy procedure (referenced by Scenario B rebuild)
- [chain#513](https://github.com/ligate-io/ligate-chain/issues/513): this runbook's tracking issue
- [chain#360](https://github.com/ligate-io/ligate-chain/issues/360): pre-mainnet security gap tracker
- [chain#273](https://github.com/ligate-io/ligate-chain/issues/273): state-snapshot publishing (improves Scenario A's worst case)
