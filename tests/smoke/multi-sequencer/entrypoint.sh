#!/bin/sh
# Container entrypoint for the multi-sequencer smoke. Renders the
# rollup.toml template with the per-container env vars, then execs
# `ligate-node` against the rendered file.
#
# This is intentionally simpler than the production
# `ops/cloud-init/render-rollup-toml.sh` (no GCP Secret Manager;
# password is supplied via docker-compose env var). The shape of the
# rendered postgres_config block matches what the prod script
# produces.

set -eu

: "${LIGATE_NODE_ID:?LIGATE_NODE_ID env var is required}"
: "${LIGATE_BIND_HOST:?}"
: "${LIGATE_BIND_PORT:?}"
: "${LIGATE_STORAGE_PATH:?}"
: "${LIGATE_METRICS_BIND_HOST:?}"
: "${LIGATE_METRICS_BIND_PORT:?}"
: "${POSTGRES_HOST:?}"
: "${POSTGRES_PORT:?}"
: "${POSTGRES_DB:?}"
: "${POSTGRES_USER:?}"
: "${POSTGRES_PASSWORD:?}"

TEMPLATE="/templates/rollup.toml.template"
RENDERED="/var/lib/ligate/rollup.toml"

mkdir -p "$(dirname "$RENDERED")" "$LIGATE_STORAGE_PATH" /var/lib/ligate/da

# `envsubst` substitutes only the named env vars. If we used the
# default behaviour, any `$0`-style positional refs would explode. We
# enumerate the vars we expect to substitute so a typo in the template
# can't leak unintended env into the rendered config.
SUBST_VARS='${LIGATE_NODE_ID} ${LIGATE_BIND_HOST} ${LIGATE_BIND_PORT}'
SUBST_VARS="$SUBST_VARS \${LIGATE_STORAGE_PATH} \${LIGATE_METRICS_BIND_HOST} \${LIGATE_METRICS_BIND_PORT}"
SUBST_VARS="$SUBST_VARS \${POSTGRES_HOST} \${POSTGRES_PORT} \${POSTGRES_DB}"
SUBST_VARS="$SUBST_VARS \${POSTGRES_USER} \${POSTGRES_PASSWORD}"

# busybox envsubst (alpine) doesn't accept the -i flag but takes the
# var list as a single positional arg.
envsubst "$SUBST_VARS" < "$TEMPLATE" > "$RENDERED"

# Sanity-check: the rendered toml must have the postgres_config block
# with the right node_id.
if ! grep -q "^node_id = \"$LIGATE_NODE_ID\"$" "$RENDERED"; then
  echo "entrypoint: render failed; node_id substitution did not land" >&2
  echo "--- rendered rollup.toml ---" >&2
  cat "$RENDERED" >&2
  exit 1
fi

echo "entrypoint: rendered rollup.toml for node_id=$LIGATE_NODE_ID" >&2
echo "entrypoint: launching ligate-node against $RENDERED" >&2

# `--rollup-config-path` and `--genesis-config-dir` are inherited
# from the chain's CLI. Mock DA is the implicit default; we don't
# pass `--da-layer celestia` here.
exec /usr/local/bin/ligate-node \
  --rollup-config-path "$RENDERED" \
  --genesis-config-dir /var/lib/ligate/genesis \
  --mode sequencer
