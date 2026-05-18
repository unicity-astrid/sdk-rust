#!/usr/bin/env bash
# Sync host/astrid-capsule.wit from the canonical unicity-astrid/wit submodule
# (at contracts/) into the published astrid-sys crate (at astrid-sys/wit/).
#
# Why this exists: astrid-sys is published to crates.io. cargo package
# can only include files inside the crate's directory, so the WIT file
# has to physically live at astrid-sys/wit/astrid-capsule.wit. The
# canonical source of truth is contracts/host/astrid-capsule.wit; this
# script copies it across. CI lint compares the two and fails if they
# diverge.
#
# Usage: scripts/sync-host-wit.sh
# To verify (no copy):   scripts/sync-host-wit.sh --check

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/contracts/host/astrid-capsule.wit"
DST="$ROOT/astrid-sys/wit/astrid-capsule.wit"

if [[ ! -f "$SRC" ]]; then
  echo "sync-host-wit: source not found: $SRC" >&2
  echo "sync-host-wit: did you forget 'git submodule update --init'?" >&2
  exit 1
fi

if [[ "${1:-}" == "--check" ]]; then
  if ! diff -q "$SRC" "$DST" >/dev/null 2>&1; then
    echo "sync-host-wit: $DST is out of sync with $SRC" >&2
    echo "sync-host-wit: run scripts/sync-host-wit.sh to fix" >&2
    diff "$SRC" "$DST" >&2 || true
    exit 1
  fi
  echo "sync-host-wit: in sync"
  exit 0
fi

cp "$SRC" "$DST"
echo "sync-host-wit: $DST ← $SRC"
