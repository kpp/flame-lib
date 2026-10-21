#!/usr/bin/env bash
set -euo pipefail
umask 077

APP="$(cd "$(dirname "$0")" && pwd)"
if [ "$#" -gt 1 ]; then
  echo "Usage: $0 [database-path]" >&2
  exit 2
fi

cd "$APP"
exec cargo run --quiet --locked --manifest-path "$APP/Cargo.toml" -- --reset-db "$@"
