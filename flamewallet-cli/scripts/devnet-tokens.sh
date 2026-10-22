#!/usr/bin/env bash
# Starts a throwaway devnet node and plays with minted tokens. The genesis
# wallet A funds two minting addresses of its own, 2/0 for "soot" and 2/1
# for "ash", mints from them, and pays the tokens around with a fresh
# wallet B. Every mint and every send from a minting address returns its
# change there, so A keeps minting without topping anything up. B mints a
# "soot" of its own, which is another flavor. Checks every balance on the
# way and exits non-zero on the first thing that is wrong. Run from
# anywhere:
#
#   flamewallet-cli/scripts/devnet-tokens.sh
#
# Builds with -j1 by default; set CARGO_BUILD_JOBS to change that. Set
# KEEP=1 to keep the scratch directory and print its path; PORT changes
# the RPC port (default 18546).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
port="${PORT:-18546}"
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
# The confirmed balance of one flavor (64 hex digits, or FLM), 0 if none.
balance() {
    local flavor="$1"; shift
    [[ "$flavor" == FLM ]] && flavor="$flame"
    "$wallet" "$@" --json balance | python3 -c "
import json, sys
balances = json.load(sys.stdin)['balances']
print(next((b['confirmed'] for b in balances if b['flavor'] == sys.argv[1]), '0'))" "$flavor"
}
# The confirmed amount of one flavor held at one path, 0 if none.
held_at() {
    local path="$1" flavor="$2"; shift 2
    [[ "$flavor" == FLM ]] && flavor="$flame"
    "$wallet" "$@" --json coins | python3 -c "
import json, sys
branch, index = map(int, sys.argv[1].split('/'))
print(sum(int(c['qty']) for c in json.load(sys.stdin)['coins']
          if (c['branch'], c['index']) == (branch, index) and c.get('flavor') == sys.argv[2]
          and c['height'] is not None))" "$path" "$flavor"
}
expect() { [[ "$2" == "$3" ]] || { echo "FAIL: $1: expected $3, got $2" >&2; exit 1; }; }
say() { printf '\n== %s\n' "$*"; }
show() { echo "-- $1"; "$wallet" "${@:2}" balance | sed 's/^/   /'; }

# The genesis allocation is under the seed a1 x 64.
printf 'a1%.0s' $(seq 64) | "$wallet" "${a[@]}" restore --seed >/dev/null
"$wallet" "${b[@]}" new >/dev/null 2>&1
a_address="$("$wallet" "${a[@]}" --json address | field "[0]['address']")"
b_address="$("$wallet" "${b[@]}" --json address | field "[0]['address']")"
flame="$("$wallet" "${a[@]}" --json balance | field "['balances'][0]['flavor']")"

say "A hands out two minting addresses and funds each with 10 Flame"
a_soot_mint="$("$wallet" "${a[@]}" --json address --path 2/0 | field "['address']")"
a_ash_mint="$("$wallet" "${a[@]}" --json address --path 2/1 | field "['address']")"
"$wallet" "${a[@]}" send "$a_soot_mint" 10 --wait
"$wallet" "${a[@]}" send "$a_ash_mint" 10 --wait
soot="$("$wallet" "${a[@]}" flavor soot --from 2/0)"
ash="$("$wallet" "${a[@]}" flavor ash --from 2/1)"
echo "soot = $soot  (minted from 2/0)"
echo "ash  = $ash  (minted from 2/1)"

say "A mints 1000 soot; it and the Flame change stay at 2/0"
"$wallet" "${a[@]}" issue 1000 --tag soot --from 2/0 --wait
expect "soot at 2/0" "$(held_at 2/0 "$soot" "${a[@]}")" 1000
expect "Flame at 2/0" "$(held_at 2/0 FLM "${a[@]}")" 999999000

say "A mints 50 ash straight to B, with a memo"
"$wallet" "${a[@]}" issue 50 --tag ash --from 2/1 --to "$b_address" --memo "ash for you" --wait
expect "B's ash" "$(balance "$ash" "${b[@]}")" 50
expect "B's memo" "$("$wallet" "${b[@]}" --json history | field "['items'][0]['memo']")" "ash for you"
expect "Flame at 2/1" "$(held_at 2/1 FLM "${a[@]}")" 999999000

say "A pays B 300 soot, and 5 Flame for B's fees"
"$wallet" "${a[@]}" send "$b_address" 300 --flavor "$soot" --memo "some soot" --wait
"$wallet" "${a[@]}" send "$b_address" 5 --wait
expect "A's soot" "$(balance "$soot" "${a[@]}")" 700
expect "B's soot" "$(balance "$soot" "${b[@]}")" 300
expect "B's Flame" "$(balance FLM "${b[@]}")" 500000000

say "B pays 100 soot back"
"$wallet" "${b[@]}" send "$a_address" 100 --flavor "$soot" --wait
expect "A's soot" "$(balance "$soot" "${a[@]}")" 800
expect "B's soot" "$(balance "$soot" "${b[@]}")" 200

say "B cannot pay more soot than it holds"
if "$wallet" "${b[@]}" send "$a_address" 1000 --flavor "$soot" 2>"$dir/err"; then
    echo "FAIL: B overspent its soot" >&2; exit 1
fi
sed 's/^/   /' "$dir/err"

say "A mints 500 more soot from 2/0: same flavor, no top-up"
"$wallet" "${a[@]}" issue 500 --tag soot --from 2/0 --wait
expect "A's soot, one flavor" "$(balance "$soot" "${a[@]}")" 1300
expect "soot at 2/0" "$(held_at 2/0 "$soot" "${a[@]}")" 500

say "A pays B 50 soot from 2/0 alone; both kinds of change return there"
"$wallet" "${a[@]}" send "$b_address" 50 --flavor "$soot" --from 2/0 --wait
expect "soot at 2/0" "$(held_at 2/0 "$soot" "${a[@]}")" 450
expect "Flame at 2/0" "$(held_at 2/0 FLM "${a[@]}")" 999997000

say "So A mints again from 2/0 straight away"
"$wallet" "${a[@]}" issue 200 --tag soot --from 2/0 --wait
expect "soot at 2/0" "$(held_at 2/0 "$soot" "${a[@]}")" 650
expect "Flame at 2/0" "$(held_at 2/0 FLM "${a[@]}")" 999996000

say "B mints its own soot from its own 2/0: same tag, another flavor"
b_soot_mint="$("$wallet" "${b[@]}" --json address --path 2/0 | field "['address']")"
"$wallet" "${b[@]}" send "$b_soot_mint" 1 --wait
b_soot="$("$wallet" "${b[@]}" flavor soot --from 2/0)"
echo "B's soot = $b_soot"
[[ "$b_soot" != "$soot" ]] || { echo "FAIL: B's soot is A's flavor" >&2; exit 1; }
"$wallet" "${b[@]}" issue 10 --tag soot --from 2/0 --wait
expect "B's own soot" "$(balance "$b_soot" "${b[@]}")" 10
expect "B's soot from A" "$(balance "$soot" "${b[@]}")" 250

say "Where everything ended up"
show A "${a[@]}"
show B "${b[@]}"
expect "A's soot" "$(balance "$soot" "${a[@]}")" 1450
expect "A's Flame" "$(balance FLM "${a[@]}")" 99499991000
expect "B's Flame" "$(balance FLM "${b[@]}")" 499997000

echo
echo "devnet tokens: ok"
