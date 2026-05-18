#!/usr/bin/env bash
# Sync astrid-sdk/wit/astrid-contracts.wit from the canonical
# unicity-astrid/wit submodule (at contracts/interfaces/*.wit).
#
# Why this exists, in two parts:
#
#   1. astrid-sdk is published to crates.io. cargo package only includes
#      files inside the crate's directory, so the WIT input to
#      wit_events! has to physically live at
#      astrid-sdk/wit/astrid-contracts.wit.
#
#   2. wit_events! today expects ONE wit file declaring ONE package
#      containing all the interfaces. The canonical repo is laid out
#      with one package per file (astrid:context, astrid:llm, …)
#      because that's the right shape for a multi-language source of
#      truth. This script transforms the per-package canonical files
#      into the single-package bundled form the macro consumes:
#
#        - strips the leading `package astrid:<name>@x.y.z;` from each
#          file (replaced by a single `package astrid:contracts@1.0.0;`
#          header below)
#        - strips intra-canonical `use astrid:<other-pkg>/...;` lines
#          (they become intra-package references after concat — every
#          record they referenced is already in scope)
#
# The bundled file is a derived artifact; the canonical
# contracts/interfaces/*.wit set is the source of truth. CI runs
# `sync-contracts-wit.sh --check` to fail if anyone hand-edits the
# bundled file out of sync.
#
# Usage: scripts/sync-contracts-wit.sh
# Verify (no write):   scripts/sync-contracts-wit.sh --check

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC_DIR="$ROOT/contracts/interfaces"
DST="$ROOT/astrid-sdk/wit/astrid-contracts.wit"

if [[ ! -d "$SRC_DIR" ]]; then
  echo "sync-contracts-wit: source dir not found: $SRC_DIR" >&2
  echo "sync-contracts-wit: did you forget 'git submodule update --init'?" >&2
  exit 1
fi

# Stable interface order: types must come first (it's foundational and
# referenced by others), then alphabetical. Without this the diff churns
# every time `ls` returns a different order on a different filesystem.
declare -a ordered
ordered+=("$SRC_DIR/types.wit")
while IFS= read -r f; do
  ordered+=("$f")
done < <(find "$SRC_DIR" -name '*.wit' ! -name 'types.wit' | sort)

generate() {
  cat <<'HEADER'
// AUTO-GENERATED — DO NOT EDIT.
//
// Concatenated from contracts/interfaces/*.wit by
// scripts/sync-contracts-wit.sh.
//
// Source of truth: unicity-astrid/wit (git submodule at contracts/).
// Run `scripts/sync-contracts-wit.sh` after pulling the submodule to
// regenerate this file; CI fails if it drifts from the canonical set.

package astrid:contracts@1.0.0;

HEADER

  for src in "${ordered[@]}"; do
    name=$(basename "$src" .wit)
    echo "// ── $name ──────────────────────────────────────────────────────────"
    # Transform (sed portable across BSD / GNU):
    #   - drop the `package astrid:<x>@<ver>;` declaration (one combined
    #     header is emitted above)
    #   - rewrite `use astrid:<pkg>/<iface>.{…};` into `use <iface>.{…};`
    #     — after concat all interfaces live in the same package, so a
    #     cross-package reference becomes a same-package cross-interface
    #     reference. The interface name is the second `/` segment.
    sed -E \
      -e '/^[[:space:]]*package[[:space:]]+astrid:/d' \
      -e 's|^([[:space:]]*)use[[:space:]]+astrid:[^/]+/([^.]+)\.|\1use \2.|' \
      "$src"
    echo
  done
}

if [[ "${1:-}" == "--check" ]]; then
  tmp=$(mktemp)
  trap 'rm -f "$tmp"' EXIT
  generate > "$tmp"
  if ! diff -q "$tmp" "$DST" >/dev/null 2>&1; then
    echo "sync-contracts-wit: $DST is out of sync with $SRC_DIR" >&2
    echo "sync-contracts-wit: run scripts/sync-contracts-wit.sh to fix" >&2
    diff "$tmp" "$DST" >&2 || true
    exit 1
  fi
  echo "sync-contracts-wit: in sync"
  exit 0
fi

generate > "$DST"
echo "sync-contracts-wit: $DST ← $SRC_DIR/*.wit ($(ls "$SRC_DIR" | wc -l | tr -d ' ') files)"
