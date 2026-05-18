#!/usr/bin/env bash
# Install swagger-ui-dist@5+ to /var/www/swagger-ui/ for Caddy to serve
# at /v1/swagger-ui/*. Required because the chain-bundled swagger-ui
# can't render OpenAPI 3.1 specs (utoipa hardcodes "3.1.0" as the
# version string and the bundled UI refuses any 3.1.x declaration).
#
# Idempotent: safe to re-run. Pin the version at the top; bump when
# you want a newer swagger-ui release.
#
# Run as a user with sudo:
#   ./ops/install/swagger-ui-dist.sh
#
# Drop the dependency on this script + its assets once Sovereign SDK
# bumps its bundled swagger-ui to 5+. Tracking: ligate-io/ligate-chain#394.

set -euo pipefail

# ----- Config ----------------------------------------------------------------

SWAGGER_UI_VERSION="${SWAGGER_UI_VERSION:-5.17.14}"
INSTALL_DIR="${INSTALL_DIR:-/var/www/swagger-ui}"
OWNER_USER="${OWNER_USER:-caddy}"
OWNER_GROUP="${OWNER_GROUP:-caddy}"
SPEC_URL="${SPEC_URL:-/openapi-v3.json}"

# ----- Sanity checks --------------------------------------------------------

if ! command -v curl >/dev/null; then
    echo "curl required, install it first" >&2
    exit 1
fi
if ! command -v tar >/dev/null; then
    echo "tar required" >&2
    exit 1
fi
if [ "$(id -u)" -ne 0 ] && ! sudo -n true 2>/dev/null; then
    echo "this script needs sudo (password-less or run as root)" >&2
    exit 1
fi

# ----- Download + install ---------------------------------------------------

echo "==> installing swagger-ui-dist@${SWAGGER_UI_VERSION} → ${INSTALL_DIR}"
sudo mkdir -p "${INSTALL_DIR}"

TARBALL_URL="https://github.com/swagger-api/swagger-ui/archive/refs/tags/v${SWAGGER_UI_VERSION}.tar.gz"
TMPDIR=$(mktemp -d)
trap 'rm -rf "${TMPDIR}"' EXIT

echo "    fetching ${TARBALL_URL}"
curl -fsSL -o "${TMPDIR}/swagger-ui.tgz" "${TARBALL_URL}"

# --strip-components=2 drops `swagger-ui-<version>/dist/` so the dist contents
# land directly under INSTALL_DIR.
echo "    extracting"
sudo tar -xzf "${TMPDIR}/swagger-ui.tgz" \
    --strip-components=2 \
    -C "${INSTALL_DIR}" \
    "swagger-ui-${SWAGGER_UI_VERSION}/dist"

# ----- Customize swagger-initializer.js -------------------------------------

# Point swagger-ui at /openapi-v3.json (Caddy's `handle /openapi-v3.json`
# block internally rewrites that path to /v1/openapi-v3.json so the chain
# serves it). tryItOutEnabled exposes the Try-it-out button for partners
# to issue requests directly from the browser.
echo "==> writing swagger-initializer.js (spec url=${SPEC_URL})"
sudo tee "${INSTALL_DIR}/swagger-initializer.js" >/dev/null <<EOF
window.onload = function() {
  window.ui = SwaggerUIBundle({
    url: "${SPEC_URL}",
    dom_id: "#swagger-ui",
    deepLinking: true,
    layout: "StandaloneLayout",
    presets: [
      SwaggerUIBundle.presets.apis,
      SwaggerUIStandalonePreset
    ],
    plugins: [SwaggerUIBundle.plugins.DownloadUrl],
    tryItOutEnabled: true
  });
};
EOF

# ----- Ownership + permissions ---------------------------------------------

echo "==> chown ${OWNER_USER}:${OWNER_GROUP}"
# Some hosts run Caddy under a different user; fall back to a world-readable
# mode if the configured owner doesn't exist.
if id -u "${OWNER_USER}" >/dev/null 2>&1; then
    sudo chown -R "${OWNER_USER}:${OWNER_GROUP}" "${INSTALL_DIR}"
else
    echo "    (user ${OWNER_USER} not found; chmod a+r instead)"
    sudo chmod -R a+r "${INSTALL_DIR}"
fi

# ----- Verify ---------------------------------------------------------------

echo "==> verify"
if [ ! -f "${INSTALL_DIR}/index.html" ] \
   || [ ! -f "${INSTALL_DIR}/swagger-ui-bundle.js" ] \
   || [ ! -f "${INSTALL_DIR}/swagger-initializer.js" ]; then
    echo "    MISSING expected files in ${INSTALL_DIR}/" >&2
    ls -la "${INSTALL_DIR}/" >&2
    exit 1
fi
echo "    OK: ${INSTALL_DIR}/ has index.html + swagger-ui-bundle.js + swagger-initializer.js"
echo ""
echo "==> Done. Caddy should serve from ${INSTALL_DIR} at /v1/swagger-ui/* per ops/caddy/Caddyfile."
echo "    Reload Caddy if its config changed: sudo systemctl reload caddy"
