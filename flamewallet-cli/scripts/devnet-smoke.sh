#!/usr/bin/env bash
# Starts a throwaway devnet node, restores the genesis wallet, pays a fresh
# wallet, pays part of it back, and checks both balances. Exits non-zero on
# the first thing that is wrong. Run from anywhere:
#
#   flamewallet-cli/scripts/devnet-smoke.sh
#
# Builds with -j1 by default; set CARGO_BUILD_JOBS to change that. Set
# KEEP=1 to keep the scratch directory and print its path.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
port="${PORT:-18545}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}"

cargo build --manifest-path "$root/Cargo.toml" -p flamed --features devnet
cargo build --manifest-path "$root/Cargo.toml" -p flamewallet-cli
flamed="$root/target/debug/flamed"
wallet="$root/target/debug/flamewallet"

dir="$(mktemp -d)"
node_pid=""
cleanup() {
    [[ -n "$node_pid" ]] && kill "$node_pid" 2>/dev/null && wait "$node_pid" 2>/dev/null || true
    if [[ "${KEEP:-}" == 1 ]]; then echo "kept $dir"; else rm -rf "$dir"; fi
}
trap cleanup EXIT

"$flamed" genesis --chainparams "$root/flamed/configs/chainparams.toml" --out "$dir/genesis.json"
cat >"$dir/flamed.toml" <<EOF
data_dir = "$dir/data"
genesis = "$dir/genesis.json"
rpc_bind = "127.0.0.1:$port"
block_interval_secs = 2
minimum_fee = 0
EOF
"$flamed" run --config "$dir/flamed.toml" >"$dir/node.log" 2>&1 &
node_pid=$!

export FLAMEWALLET_NODE="http://127.0.0.1:$port"
for _ in $(seq 50); do grep -q "serving JSON-RPC" "$dir/node.log" 2>/dev/null && break; sleep 0.2; done

a=(--wallet "$dir/a.json")
b=(--wallet "$dir/b.json")
field() { python3 -c "import json,sys; v=json.load(sys.stdin); print(eval('v' + sys.argv[1]))" "$1"; }
# Flame is always listed first.
flame_balance() { "$wallet" "$@" --json balance | field "['balances'][0]['confirmed']"; }
expect() { [[ "$2" == "$3" ]] || { echo "FAIL: $1: expected $3, got $2" >&2; exit 1; }; }

# The genesis allocation is under the seed a1 x 64.
printf 'a1%.0s' $(seq 64) | "$wallet" "${a[@]}" restore --seed >/dev/null
"$wallet" "${b[@]}" new >/dev/null 2>&1
b_address="$("$wallet" "${b[@]}" --json address | field "[0]['address']")"
a_address="$("$wallet" "${a[@]}" --json address | field "[0]['address']")"

expect "A at genesis" "$(flame_balance "${a[@]}")" 100000000000

"$wallet" "${a[@]}" send "$b_address" 12.5 --memo smoke --wait >/dev/null
expect "B after receiving" "$(flame_balance "${b[@]}")" 1250000000
expect "B's memo" "$("$wallet" "${b[@]}" --json history | field "['items'][0]['memo']")" smoke

"$wallet" "${b[@]}" send "$a_address" 2 --wait >/dev/null
expect "B after paying back" "$(flame_balance "${b[@]}")" 1049999000
expect "A after both" "$(flame_balance "${a[@]}")" 98949999000

echo "devnet smoke: ok"
