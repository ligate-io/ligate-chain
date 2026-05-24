#!/usr/bin/env bash
# Renders the per-instance rollup.toml for the multi-sequencer DbElected
# topology (chain #82, sub-issue #412).
#
# Reads the canonical `localnet/rollup.toml` from disk, fetches the
# Cloud SQL app-role password from GCP Secret Manager, substitutes
# `${LIGATE_NODE_ID}` and `${POSTGRES_PASSWORD}` into the commented
# `[sequencer.preferred.postgres_config]` block, and writes the
# activated config to the path the chain process will consume.
#
# Designed to run as a systemd `ExecStartPre=` step before
# `ligate-node.service`. Idempotent: running multiple times produces
# the same output for the same env inputs.
#
# Required env:
#   LIGATE_NODE_ID         distinct per instance (e.g. "ligate-1")
#   GCP_PROJECT            project hosting the secret
#   POSTGRES_SECRET_NAME   defaults to cloudsql-ligate-sequencer-db-app
#
# Optional env:
#   SOURCE_TOML            defaults to /opt/ligate/localnet/rollup.toml
#   TARGET_TOML            defaults to /var/lib/ligate/rollup.toml
#   POSTGRES_HOST          defaults to 10.123.0.2 (Cloud SQL private IP)
#   POSTGRES_PORT          defaults to 5432
#   POSTGRES_DB            defaults to ligate_sequencer
#   POSTGRES_USER          defaults to ligate_sequencer
#
# Activation note: this script alone does not enable DbElected mode.
# It only activates the commented block in the source toml when the
# env vars are set. Without `LIGATE_NODE_ID` set, the script refuses
# to run so a typo in cloud-init can't accidentally produce an
# un-templated config.

set -euo pipefail

LIGATE_NODE_ID="${LIGATE_NODE_ID:?LIGATE_NODE_ID env var is required (distinct per instance, e.g. ligate-1)}"
GCP_PROJECT="${GCP_PROJECT:?GCP_PROJECT env var is required}"
POSTGRES_SECRET_NAME="${POSTGRES_SECRET_NAME:-cloudsql-ligate-sequencer-db-app}"

SOURCE_TOML="${SOURCE_TOML:-/opt/ligate/localnet/rollup.toml}"
TARGET_TOML="${TARGET_TOML:-/var/lib/ligate/rollup.toml}"
POSTGRES_HOST="${POSTGRES_HOST:-10.123.0.2}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_DB="${POSTGRES_DB:-ligate_sequencer}"
POSTGRES_USER="${POSTGRES_USER:-ligate_sequencer}"

echo "render-rollup-toml: source=$SOURCE_TOML target=$TARGET_TOML node_id=$LIGATE_NODE_ID" >&2

# Validate the source exists and is readable
if [[ ! -r "$SOURCE_TOML" ]]; then
  echo "render-rollup-toml: source $SOURCE_TOML not readable" >&2
  exit 1
fi

# Fetch the postgres app password from Secret Manager.
# Uses the VM's instance service account (granted secretAccessor on
# the secret); no static keys on disk.
POSTGRES_PASSWORD="$(gcloud secrets versions access latest \
  --secret="$POSTGRES_SECRET_NAME" \
  --project="$GCP_PROJECT" 2>/dev/null)"

if [[ -z "$POSTGRES_PASSWORD" ]]; then
  echo "render-rollup-toml: failed to fetch $POSTGRES_SECRET_NAME from Secret Manager (project=$GCP_PROJECT). Check VM SA has roles/secretmanager.secretAccessor." >&2
  exit 1
fi

# Build the connection string. URL-encode the password (basic case;
# our generator avoids `:/?#[]@` so a direct interpolation is safe
# for the standard rollup-of-the-mill alphanumeric + `_.-` charset).
POSTGRES_CONN="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${POSTGRES_DB}?sslmode=require"

# Ensure target dir exists.
mkdir -p "$(dirname "$TARGET_TOML")"

# Render: copy source verbatim, then uncomment + substitute the
# postgres_config block. We use a marker-based sed transform so that
# accidental future edits to the comment block don't break us
# silently: the source must contain `# SEAM: multi-sequencer activation block` exactly.
if ! grep -q "^# SEAM: multi-sequencer activation block" "$SOURCE_TOML"; then
  echo "render-rollup-toml: source $SOURCE_TOML missing SEAM marker; aborting (rollup.toml schema drift?)" >&2
  exit 1
fi

# Strip leading `# ` from the four config-active lines (the three
# `postgres_config` keys + the leading section header). Leaves the
# leader_election overrides commented (we want SDK defaults unless
# explicitly tuned).
sed \
  -e 's|^# \(\[sequencer\.preferred\.postgres_config\]\)$|\1|' \
  -e "s|^# postgres_connection_string = \"postgresql://ligate_sequencer:\${POSTGRES_PASSWORD}@10\\.123\\.0\\.2:5432/ligate_sequencer?sslmode=require\"$|postgres_connection_string = \"${POSTGRES_CONN}\"|" \
  -e "s|^# node_id = \"\${LIGATE_NODE_ID}\"$|node_id = \"${LIGATE_NODE_ID}\"|" \
  -e 's|^# node_role = "DbElected"$|node_role = "DbElected"|' \
  "$SOURCE_TOML" > "$TARGET_TOML.tmp"

# Sanity-check: must have an activated postgres_config block now.
if ! grep -q '^\[sequencer\.preferred\.postgres_config\]$' "$TARGET_TOML.tmp"; then
  echo "render-rollup-toml: render failed; activated block not produced. Inspect $TARGET_TOML.tmp manually." >&2
  exit 1
fi
if ! grep -q "^node_id = \"${LIGATE_NODE_ID}\"$" "$TARGET_TOML.tmp"; then
  echo "render-rollup-toml: render failed; node_id substitution did not land." >&2
  exit 1
fi

mv "$TARGET_TOML.tmp" "$TARGET_TOML"
chmod 600 "$TARGET_TOML"

echo "render-rollup-toml: wrote $TARGET_TOML (mode 600)" >&2
