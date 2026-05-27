# Key Management, Design Decision

**Status:** decision recorded for pre-mainnet implementation; devnet stays on file-backed keys
**Scope:** how Ligate Chain stores and uses the operator + DA wallet keys for `ligate-sequencer` and downstream services
**Applies to:** the operator key at `~/.ligate-keys/devnet-2/operator.key`, the Celestia DA signer key in GCP Secret Manager (`ligate-celestia-signer-key`), and any future per-validator keys once the staking module ([#50](https://github.com/ligate-io/ligate-chain/issues/50)) ships
**Tracks:** [chain#514](https://github.com/ligate-io/ligate-chain/issues/514), child of [chain#360](https://github.com/ligate-io/ligate-chain/issues/360)

---

## TL;DR

**Pick GCP Cloud KMS with HSM-tier protection** for operator + DA keys. Stand up a thin local signer-proxy (`ligate-signer-proxy`) that wraps `crypto.signAsymmetric()` calls against a KMS keyring. The chain binary speaks to the proxy over a Unix domain socket, never sees the private key material.

**Migrate devnet-2 keys as the dry run** before mainnet. Devnet stays on the existing file + Secret Manager setup until the proxy is hardened.

**Out of scope:** AWS CloudHSM (cross-cloud, too expensive for this stage), HashiCorp Vault (more ops surface for marginal gain), YubiHSM (physical-device model does not fit remote VMs).

---

## 1. What we have today

| Key | Stored | Used by | Blast radius if compromised |
|-----|--------|---------|------------------------------|
| Operator key (`operator.key`) | Laptop filesystem, macOS Keychain, GCP Secret Manager | `ligate-sequencer`, `ligate-cli` (faucet drip), genesis treasury holder | Adversary signs faucet drips, drains treasury balance, can sign batches if the operator role overlaps the sequencer signing key |
| Celestia DA signer | GCP Secret Manager (`ligate-celestia-signer-key`) | `ligate-sequencer` blob-submission path | Adversary drains TIA balance, blocks DA submission via fee-bidding wars. Chain state is safe. |
| Faucet signer | Faucet repo's own Secret Manager binding | `faucet-bot` only | Faucet-only loss surface; tracked separately in the faucet repo |

The devnet-2 setup is fine for "we are the only operator and we control the laptop." It is not fine for mainnet, where a key compromise should require the adversary to break a hardware boundary, not exfiltrate a file.

## 2. Requirements

The chosen backing store must satisfy:

**R1. Hardware-rooted protection of private key material.** Private bytes never leave the secure boundary in cleartext. Signing happens inside the boundary; what comes out is the signature.

**R2. Latency budget for hot signing.** Sequencer signs every batch (~1 batch per slot, ~1s on devnet). DA signer signs every blob submission. KMS round-trip needs to be well under the slot budget. Target: p50 < 50ms, p99 < 150ms.

**R3. Audit-grade access logs.** Every signing operation produces a log line with caller identity, key id, timestamp. Required for IR forensics post-incident.

**R4. Survives single-VM loss.** Key material is not VM-local. A wiped sequencer VM can spin up a replacement, authenticate to the KMS, resume signing.

**R5. Same-cloud + same-IAM-plane as the rest of our infra.** Everything else is GCP. Adding a second cloud's IAM model for a single key store is operational overhead with no security gain.

**R6. Migration path from the current file + Secret Manager setup.** Drop-in for the signer abstraction inside `ligate-sequencer`; no breaking change for downstream consumers.

## 3. Options considered

### Option 1, GCP Cloud KMS (recommended)

Native GCP service. Software-backed by default; HSM-backed available as a key option (`PROTECTION_LEVEL=HSM`, FIPS 140-2 Level 3, the bytes physically live in a Google-operated HSM).

**Cost:** $1.00/key/month (HSM tier, asymmetric ed25519) + $0.06/10k signing operations. For operator + DA at one signing op per second: ~$3/month per key. Plus a few cents per million ops on signing volume. Effectively negligible.

**Latency:** Median 30-40ms from a GCP VM in the same region. Well inside R2.

**Ops surface:** IAM-managed, audit-logged via Cloud Audit Logs. Granular per-key permissions. No service to run; KMS is fully managed.

**Migration shape:** Generate ed25519 key in KMS, export public key, register the new public key in genesis (re-genesis event), rotate downstream services to call the signer-proxy. See `docs/development/runbooks/operator-key-rotation.md` for the procedure.

**Risks:** Cloud-provider lock-in for the signing API. Mitigated because the signer-proxy abstraction sits between the chain and KMS; swapping providers later is a proxy-side change, not a chain-side change.

### Option 2, AWS CloudHSM

Dedicated HSM instance, FIPS 140-2 Level 3. Higher assurance than KMS HSM-tier, also higher cost.

**Cost:** $1.50/hour per HSM, ~$1,080/month minimum. HA setup needs at least 2 HSMs across AZs for ~$2,160/month.

**Latency:** Inside AWS region: ~10-20ms. From GCP via cross-cloud: 80-150ms; that pushes against R2 for the slot budget.

**Ops surface:** Self-managed cluster (`cloudhsmv2` SDK), key backup procedures, AZ rebalancing. More day-2 work than KMS.

**Verdict:** Too expensive for current stage. Reconsider post-mainnet if (a) we move to AWS-native infra, or (b) regulatory pressure forces dedicated-HSM assurance over multi-tenant HSM.

### Option 3, HashiCorp Vault

Open source secret manager. Can use cloud HSM for auto-unseal; signs via Transit secrets engine.

**Cost:** OSS is free; HCP Vault starts at $0.03/hour/cluster = ~$22/month for a single dev-tier cluster. Self-host adds VM + ops time.

**Latency:** Vault server within the same VPC: 20-50ms. Acceptable.

**Ops surface:** Vault server itself (HA, backups, unseal procedures), Transit engine policies, IAM integration. Adds a moving part the team has to learn and run.

**Why we are not picking this:** We are GCP-only. Vault shines when you need a multi-cloud abstraction or want to keep secrets out of the cloud provider's KMS. Neither applies. KMS does the same job with less ops surface for our setup.

**Reconsider trigger:** if we go multi-cloud (e.g. AWS-side ZK prover infrastructure that needs the same signing key), Vault becomes the right shape.

### Option 4, YubiHSM 2

Physical USB device, $650 per HSM, FIPS 140-2 Level 3.

**Cost:** $650 per device, no recurring fee. Two devices for redundancy = $1,300 one-time.

**Latency:** Microseconds (USB-local) on the host that owns the device.

**Ops surface:** Physical presence at the signing host. Does not fit a remote-VM-only operator model. To use a YubiHSM with a GCP VM, you would need either (a) a self-hosted machine somewhere that talks to GCP, or (b) a remote-attestation setup, neither of which we currently have.

**Verdict:** Wrong model for our deployment. Useful for a self-hosted prover in v2+ if we go that route.

### Option 5, Status quo (file + GCP Secret Manager)

Current devnet setup. Keys live as files on disk, mirrored into GCP Secret Manager. Chain binary loads via env var pointing at a path.

**Why this is unacceptable for mainnet:** R1 fails. A laptop compromise, a leaked crash dump, a misconfigured backup, or an exfiltrated Secret Manager export gives the adversary the raw bytes. There is no hardware boundary.

**Why this is acceptable for devnet:** the threat model is "we have to dogfood the runtime before mainnet"; key compromise on devnet is a redo cost, not a real loss.

## 4. Recommendation

**Pick GCP Cloud KMS with HSM-tier protection.**

| Criterion | KMS verdict |
|-----------|-------------|
| R1 hardware-rooted | ✅ HSM-tier keys |
| R2 latency budget | ✅ p50 ~35ms, p99 well under 150ms |
| R3 audit logs | ✅ Cloud Audit Logs admin + data access |
| R4 survives VM loss | ✅ Keys are not VM-resident |
| R5 same cloud + IAM | ✅ Native GCP |
| R6 migration path | ✅ Signer-proxy abstraction |
| Cost | $3-5/month per key, lowest of the hardware-backed options |
| Ops surface | Lowest |

## 5. Signer-proxy design sketch

A small Rust binary `ligate-signer-proxy` runs alongside `ligate-sequencer` on the same VM. The chain talks to the proxy over a Unix domain socket (no TCP, no remote auth needed):

```
ligate-sequencer  ────[uds]────►  ligate-signer-proxy  ────[grpc]────►  GCP Cloud KMS
                                  (ADC via VM service account)
```

### Why the proxy exists

- Isolates the KMS SDK + retry logic + circuit breaker from the chain binary.
- Lets us swap KMS for Vault / CloudHSM / file later without touching `ligate-sequencer`.
- Per-request audit logging happens in one place.
- Restart of the proxy does not restart the chain.

### Wire shape

The proxy exposes a tiny RPC:

```rust
service Signer {
  rpc Sign(SignRequest) returns (SignResponse);
  rpc Pubkey(PubkeyRequest) returns (PubkeyResponse);
}

message SignRequest {
  string key_id = 1;       // e.g. "operator" | "da"
  bytes payload = 2;       // raw bytes to sign
}

message SignResponse {
  bytes signature = 1;
}
```

The chain calls `Sign(key_id="operator", payload=batch_hash)` once per batch. The proxy maps `key_id` to a fully-qualified KMS resource and calls `asymmetricSign`.

### Config shape

```toml
# rollup.toml
[signer]
mode = "proxy"                # "file" for devnet, "proxy" for mainnet
socket_path = "/run/ligate-signer-proxy.sock"
```

### Failure semantics

- Proxy down → chain refuses to start (no fallback to file in proxy mode).
- KMS unreachable → proxy returns a transient error; chain retries up to N times then halts batch production with a clear log line.
- Proxy crashes mid-batch → chain aborts the batch, logs, retries next slot.

## 6. Migration plan (devnet-2 dry run, then mainnet)

1. **Build `ligate-signer-proxy`.** New crate at `crates/signer-proxy`. Reuses the existing `Signer` trait in `crates/rollup/src/signer.rs` with a `Proxy` variant alongside `File`.
2. **KMS keyring setup.** Create `ligate-mainnet-keys` keyring with two HSM-tier keys: `operator-v1` and `da-signer-v1`. Document the `gcloud kms` commands in the rotation runbook.
3. **Devnet-2 dry run.** Migrate devnet-2 to proxy mode behind a feature flag. Run for one week. Watch metrics: signing latency p50/p99, error rate, audit log volume.
4. **Operator key rotation runbook end-to-end test.** Generate a fresh KMS key, run Procedure A from the runbook on the devnet, confirm chain stays healthy across the switch.
5. **Mainnet bootstrap with proxy mode from day 0.** No mainnet binary ever ships with `signer.mode = "file"`.

## 7. Open follow-ups

This RFC unblocks these issues but does not close them:

1. **Implement `crates/signer-proxy`.** New crate, ~500-1000 lines of Rust. Filed as a follow-up to #514.
2. **`gcloud kms` keyring + IAM bindings as Terraform.** Infra-as-code lives in `ops/terraform/keys.tf` (to be created). Filed as a follow-up to #514.
3. **Signer-proxy alerting.** Latency p99, error rate, audit-log-volume drop. Lives in `docs/development/runbooks/alerts/` once the alerting wiring from [#318](https://github.com/ligate-io/ligate-chain/issues/318) is in place.
4. **Per-validator key model post-#50.** Once the staking module ships and external validators come online, each operator needs their own key-management story. This RFC is for Ligate-Labs-operated keys; the per-validator model is its own design pass.

## 8. Cross-references

- [chain#360](https://github.com/ligate-io/ligate-chain/issues/360): parent security gap tracker (pre-mainnet umbrella)
- [chain#514](https://github.com/ligate-io/ligate-chain/issues/514): this RFC's tracking issue
- [`operator-key-rotation.md`](../development/runbooks/operator-key-rotation.md): the rotation procedure that builds on this RFC
- [`sequencer-key-rotation.md`](../development/runbooks/sequencer-key-rotation.md): chain-side ed25519 signing key rotation (distinct from operator wallet key)
- [`da-signer-rotation.md`](../development/runbooks/da-signer-rotation.md): Celestia DA wallet rotation
