# Runbook, GCP Cloud Audit Logs

**Status:** v0, the procedure for enabling project-wide Cloud Audit Logs + a retention-tier logging sink so audit trails survive a project compromise.
**Tracks:** [chain#513](https://github.com/ligate-io/ligate-chain/issues/513), child of [chain#360](https://github.com/ligate-io/ligate-chain/issues/360)

This is the forensic trail layer. Without it, "who SSH'd to the sequencer at 03:00?" and "what IAM bindings changed last week?" have no answer.

---

## What we enable

| Log type | Default | Cost | What it captures |
|----------|---------|------|------------------|
| Admin Activity | On (most services) | Free | IAM changes, resource creation/deletion, console + gcloud actions that mutate state |
| System Event | On | Free | GCE automatic actions (live migration, automatic restart) |
| Data Access | Off (most services) | Storage + ingestion at standard logging rates | SSH session start, reads against Secret Manager, KMS access |
| Policy Denied | Optional | Storage | IAM denials (interesting for "someone tried to access X but lacked perm") |

We turn on **Data Access** for the services we care about: Compute Engine (SSH), Secret Manager (key reads), Cloud KMS (signing operations, once #514 ships the KMS migration), Cloud Storage (snapshot bucket access).

---

## Procedure

### Step 1, verify Admin Activity logs are flowing (5 min)

```sh
gcloud logging read 'logName=~"cloudaudit.googleapis.com%2Factivity"' \
  --limit=5 \
  --format=json | jq '.[] | {timestamp, protoPayload: .protoPayload | {methodName, resourceName}}'
```

Should return five recent Admin Activity events. If empty, Cloud Audit Logs are disabled at the project level, contact GCP support (this would be unusual; default is on).

### Step 2, enable Data Access logs for the services we care about (15 min)

Via the Console: IAM & Admin → Audit Logs → check Data Read + Data Write for: Compute Engine, Secret Manager, Cloud KMS, Cloud Storage. Save.

Via gcloud (idempotent, scripted form):

```sh
PROJECT=ligate-mainnet  # or ligate-devnet-2

cat > /tmp/audit-config.yaml <<EOF
auditConfigs:
- service: compute.googleapis.com
  auditLogConfigs:
  - logType: DATA_READ
  - logType: DATA_WRITE
- service: secretmanager.googleapis.com
  auditLogConfigs:
  - logType: DATA_READ
  - logType: DATA_WRITE
- service: cloudkms.googleapis.com
  auditLogConfigs:
  - logType: DATA_READ
  - logType: DATA_WRITE
- service: storage.googleapis.com
  auditLogConfigs:
  - logType: DATA_READ
  - logType: DATA_WRITE
EOF

# Merge into existing project IAM policy (do NOT overwrite, the
# `etag` flow below preserves IAM bindings):
gcloud projects get-iam-policy $PROJECT --format=yaml > /tmp/policy.yaml
# Hand-merge auditConfigs from /tmp/audit-config.yaml into /tmp/policy.yaml
# (yq is fine for this; or just paste under the existing root key)
gcloud projects set-iam-policy $PROJECT /tmp/policy.yaml
```

**Cost expectation:** ~$0.50/GB for storage past the free tier. Our SSH + Secret Manager + KMS traffic at devnet scale is single-digit GB/month. Negligible.

### Step 3, configure a retention-tier sink to a dedicated bucket (10 min)

The default Cloud Logging retention is 30 days for `_Default`, 400 days for `_Required` (Admin Activity). For audit work we want a separate bucket with a long retention policy and locked IAM, so a project-level compromise can not wipe the trail.

```sh
PROJECT=ligate-mainnet
REGION=us-central1
SINK_BUCKET=ligate-audit-logs-$(date +%Y)

# 1. Create a logging bucket dedicated to audit trail, with 7-year retention
gcloud logging buckets create $SINK_BUCKET \
  --location=$REGION \
  --retention-days=2557 \
  --description="Audit-log retention bucket, locked policy"

# 2. Lock the retention policy (immutable; deletion fails until policy expires)
gcloud logging buckets update $SINK_BUCKET \
  --location=$REGION \
  --locked

# 3. Create the sink. Includes both Admin Activity AND Data Access logs.
gcloud logging sinks create ligate-audit-sink \
  logging.googleapis.com/projects/$PROJECT/locations/$REGION/buckets/$SINK_BUCKET \
  --log-filter='logName=~"cloudaudit.googleapis.com"'

# 4. Grant the sink's auto-created service account write access to the bucket
SINK_WRITER=$(gcloud logging sinks describe ligate-audit-sink \
  --format='value(writerIdentity)')
gcloud logging buckets add-iam-policy-binding $SINK_BUCKET \
  --location=$REGION \
  --member=$SINK_WRITER \
  --role=roles/logging.bucketWriter
```

After this, every audit log line lands in TWO places: the default `_Required` bucket (400-day retention) AND `ligate-audit-logs-<year>` (7-year retention, locked policy). A project compromise that wipes the default bucket cannot touch the locked one.

### Step 4, verify with a known event (5 min)

```sh
# Trigger a known event
gcloud compute ssh ligate-sequencer --zone=us-central1-a --command="echo verify"

# Wait ~30s for log propagation, then check the sink bucket
gcloud logging read \
  'logName=~"cloudaudit.googleapis.com" AND protoPayload.methodName=~"v1.compute.instances.getSerialPortOutput|ssh"' \
  --bucket=$SINK_BUCKET \
  --location=$REGION \
  --limit=3 \
  --format=json | jq '.[] | {timestamp, authenticationInfo: .protoPayload.authenticationInfo.principalEmail, methodName: .protoPayload.methodName}'
```

If the SSH event shows up in the bucket query, the sink is working.

---

## Day-2 operations

### Query the audit log from incident response

The incident-response runbook references this. Common queries:

```sh
# Who SSH'd to the sequencer in the last 24h?
gcloud logging read \
  'resource.type="gce_instance" AND resource.labels.instance_id="ligate-sequencer" AND protoPayload.methodName=~"ssh"' \
  --freshness=1d --format=json | jq '.[] | {ts: .timestamp, who: .protoPayload.authenticationInfo.principalEmail}'

# Who read Secret Manager keys in the last 7 days?
gcloud logging read \
  'resource.type="secretmanager.googleapis.com/Secret" AND protoPayload.methodName=~"AccessSecret"' \
  --freshness=7d --format=json | jq '.[] | {ts: .timestamp, who: .protoPayload.authenticationInfo.principalEmail, secret: .protoPayload.resourceName}'

# Any IAM policy changes this week?
gcloud logging read \
  'protoPayload.methodName=~"SetIamPolicy"' \
  --freshness=7d --format=json | jq '.[] | {ts: .timestamp, who: .protoPayload.authenticationInfo.principalEmail, resource: .protoPayload.resourceName}'
```

### Alerting on suspicious patterns

Once the Alertmanager wiring from [#318](https://github.com/ligate-io/ligate-chain/issues/318) is in place, the high-value alerts to wire on top of the audit log:

- IAM role grant outside business hours
- Secret Manager access from an IP outside Stefan's known range
- More than N SSH sessions in a 1-hour window
- Any `delete` on the ligate-sequencer VM

Each becomes a log-based metric + Alertmanager rule. Filed as a follow-up to #513.

---

## Acceptance for this runbook

- [x] Doc lands at `docs/development/runbooks/gcp-audit-logs.md`
- [ ] Procedure executed on devnet-2 (Stefan's task; this doc is the spec)
- [ ] Verification step (SSH triggers an entry) passes
- [ ] Sink bucket exists with locked 7-year retention

---

## Cross-references

- [chain#513](https://github.com/ligate-io/ligate-chain/issues/513): this runbook's tracking issue
- [chain#360](https://github.com/ligate-io/ligate-chain/issues/360): pre-mainnet security gap tracker
- [chain#318](https://github.com/ligate-io/ligate-chain/issues/318): Alertmanager wiring (where the audit-log-based alerts will land)
- [`incident-response.md`](./incident-response.md): IR runbook that queries the audit log during incidents
- [`disaster-recovery.md`](./disaster-recovery.md): DR runbook that pulls audit-log evidence into post-mortems
