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
#   e2e/devnet-tokens.sh
#
# KEEP=1, PORT (default 18546) and CARGO_BUILD_JOBS as in devnet.sh.
port=18546
source "$(dirname "${BASH_SOURCE[0]}")/devnet.sh"

a=(--wallet "$dir/a.json")
b=(--wallet "$dir/b.json")

# The genesis allocation is under the seed a1 x 64.
printf 'a1%.0s' $(seq 64) | "$wallet" "${a[@]}" restore --seed >/dev/null
"$wallet" "${b[@]}" new >/dev/null 2>&1
a_address="$("$wallet" "${a[@]}" --json address | field "[0]['address']")"
b_address="$("$wallet" "${b[@]}" --json address | field "[0]['address']")"

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
