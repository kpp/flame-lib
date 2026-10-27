#!/usr/bin/env bash
# Restores the genesis wallet on a throwaway devnet, pays a fresh wallet
# with a memo, pays part of it back, and checks both balances. Exits
# non-zero on the first thing that is wrong. Run from anywhere:
#
#   e2e/devnet-smoke.sh
#
# KEEP=1, PORT (default 18545) and CARGO_BUILD_JOBS as in devnet.sh.
port=18545
source "$(dirname "${BASH_SOURCE[0]}")/devnet.sh"

a=(--wallet "$dir/a.json")
b=(--wallet "$dir/b.json")

# The genesis allocation is under the seed a1 x 64.
printf 'a1%.0s' $(seq 64) | "$wallet" "${a[@]}" restore --seed >/dev/null
"$wallet" "${b[@]}" new >/dev/null 2>&1
b_address="$("$wallet" "${b[@]}" --json address | field "[0]['address']")"
a_address="$("$wallet" "${a[@]}" --json address | field "[0]['address']")"

expect "A at genesis" "$(balance FLM "${a[@]}")" 1000000000000000000

"$wallet" "${a[@]}" send "$b_address" 12.5 --memo smoke --wait >/dev/null
expect "B after receiving" "$(balance FLM "${b[@]}")" 1250000000
expect "B's memo" "$("$wallet" "${b[@]}" --json history | field "['items'][0]['memo']")" smoke

"$wallet" "${b[@]}" send "$a_address" 2 --wait >/dev/null
expect "B after paying back" "$(balance FLM "${b[@]}")" 1049999000
expect "A after both" "$(balance FLM "${a[@]}")" 999999998949999000

echo "devnet smoke: ok"
