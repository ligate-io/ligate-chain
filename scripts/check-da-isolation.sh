#!/usr/bin/env bash
# Fail if Celestia-specific identifiers leak into DA-agnostic crates.
#
# The Ligate Chain rollup is designed to be swappable across DA
# layers (Mock + Celestia today; Ethereum blobs / EigenDA / homegrown
# DA later). The STF, modules, and client crates are generic over
# `S: Spec` — which carries the `Da: DaSpec` parameter transitively
# — so they should never reference a concrete DA backend by name.
#
# Only the rollup blueprint crate and Risc0 guest crates are allowed
# to know about specific DA backends, because that's where the DA
# choice gets wired into a binary.
#
# This guard catches drift cheaply: a `use sov_celestia_adapter::*;`
# in a module crate would compile fine today (the module is generic
# over `S`), but it represents a coupling we don't want and would
# turn into a real swap blocker over time.
#
# Tracking issue: ligate-io/ligate-chain#117
# Companion doc: docs/protocol/da-layers.md (architecture + adapter playbook)

set -euo pipefail

# Crates that MUST stay DA-agnostic. Anything outside this list is
# either DA-bound on purpose (the rollup blueprint, the Risc0 guest)
# or not DA-relevant (docs, scripts, tests of the rollup itself).
guarded_paths=(
  "crates/modules"
  "crates/stf"
  "crates/stf-declaration"
  "crates/client-rs"
)

# Patterns that indicate Celestia-specific coupling. Case-insensitive
# so we catch both `Celestia` types and `celestia` identifiers / paths.
# The word "namespace" alone is not matched — schema-namespace prose
# in the attestation module is unrelated to Celestia's `Namespace`.
patterns=(
  "celestia"
  "sov_celestia_adapter"
)

leaks=0
for path in "${guarded_paths[@]}"; do
  if [[ ! -d "$path" ]]; then
    echo "::warning::guarded path does not exist: $path (script may need update)"
    continue
  fi
  for pattern in "${patterns[@]}"; do
    if matches=$(grep -r -i -n --include="*.rs" --include="*.toml" "$pattern" "$path" 2>/dev/null); then
      echo "::error::DA isolation violation in $path (pattern: '$pattern'):"
      printf '%s\n' "$matches" | sed 's/^/  /'
      leaks=1
    fi
  done
done

if [[ $leaks -ne 0 ]]; then
  echo ""
  echo "::error::Celestia coupling detected in a DA-agnostic crate."
  echo "::error::These crates must stay generic over the DA layer so we can swap"
  echo "::error::Celestia for Ethereum blobs / EigenDA / a homegrown DA later."
  echo "::error::"
  echo "::error::Move the coupling into crates/rollup/, or update this guard if"
  echo "::error::the architecture has deliberately changed (see ligate-chain#117)."
  exit 1
fi

echo "DA isolation OK: no Celestia coupling in DA-agnostic crates."
