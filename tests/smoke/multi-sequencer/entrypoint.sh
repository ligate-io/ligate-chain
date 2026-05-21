#!/bin/sh
# Container entrypoint for the multi-sequencer smoke. Renders the
# rollup.toml template with the per-container env vars (via sed),
# then execs `ligate-node` against the rendered file.
#
# Uses sed because the chain image is debian-slim minimal — no
# `envsubst` (gettext-base), no curl, no wget.
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
: "${POSTGRES_HOST:?}"
: "${POSTGRES_PORT:?}"
: "${POSTGRES_DB:?}"
: "${POSTGRES_USER:?}"
: "${POSTGRES_PASSWORD:?}"

TEMPLATE="/templates/rollup.toml.template"
RENDERED="/var/lib/ligate/rollup.toml"

mkdir -p "$(dirname "$RENDERED")" "$LIGATE_STORAGE_PATH" /var/lib/ligate/da

# sed-based substitution (envsubst isn't available in the chain
# image). Each substitution uses `|` as the delimiter so values
# containing `/` (filesystem paths, connection strings) don't need
# escaping.
sed \
  -e "s|\${LIGATE_NODE_ID}|${LIGATE_NODE_ID}|g" \
  -e "s|\${LIGATE_BIND_HOST}|${LIGATE_BIND_HOST}|g" \
  -e "s|\${LIGATE_BIND_PORT}|${LIGATE_BIND_PORT}|g" \
  -e "s|\${LIGATE_STORAGE_PATH}|${LIGATE_STORAGE_PATH}|g" \
  -e "s|\${POSTGRES_HOST}|${POSTGRES_HOST}|g" \
  -e "s|\${POSTGRES_PORT}|${POSTGRES_PORT}|g" \
  -e "s|\${POSTGRES_DB}|${POSTGRES_DB}|g" \
  -e "s|\${POSTGRES_USER}|${POSTGRES_USER}|g" \
  -e "s|\${POSTGRES_PASSWORD}|${POSTGRES_PASSWORD}|g" \
  "$TEMPLATE" > "$RENDERED"

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
  --genesis-config-dir /var/lib/ligate/genesis
