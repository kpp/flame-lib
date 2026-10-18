#!/usr/bin/env bash
set -euo pipefail

APP="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$APP/../.." && pwd)"
export PATH="$ROOT/target/mint-playground-tools/bin:$PATH"

LOCKED="$(sed -n '/^name = "wasm-bindgen"$/{n;s/^version = "\(.*\)"$/\1/p;}' "$ROOT/Cargo.lock")"
FOUND="$(wasm-bindgen --version 2>/dev/null | awk '{print $2}' || true)"
if [ "$FOUND" != "$LOCKED" ]; then
  echo "Browser wallet needs wasm-bindgen $LOCKED (found ${FOUND:-none}). Install it locally with:" >&2
  echo "  cargo install wasm-bindgen-cli --version $LOCKED --locked --root \"$ROOT/target/mint-playground-tools\"" >&2
  exit 1
fi

OUT="$ROOT/target/wallet-wasm/pkg" CARGO_TARGET_DIR="$ROOT/target" \
  bash "$ROOT/flamewallet-wasm/scripts/build.sh"

cd "$APP"
exec cargo run --locked --manifest-path "$APP/Cargo.toml" -- "$@"
