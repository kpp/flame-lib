# Address formats

This document specifies Flame's human-readable actor, key and payment-address
formats. [Bech32f](bech32f.md) specifies the text codec;
[FlameKD](flamekd.md) specifies key derivation and raw cryptographic tuples.

The words MUST, MUST NOT, SHOULD and MAY specify implementation requirements.

All components use Bech32f's encoding, checksum, case and padding rules. Decoders
MUST enforce the expected type, selected network's HRPs and exact lengths from
the tables below.

## Keys and payment addresses

Encode the raw tuple defined in [FlameKD's key types](flamekd.md#key-types-and-capabilities).
After text decoding, validate it using FlameKD's
[scalar and point rules](flamekd.md#group-and-representations).
Network-specific derivation follows its
[standard wallet paths](flamekd.md#standard-wallet-derivation-paths).

| Type | Mainnet HRP | Testnet HRP | Bytes | Payload symbols | Total characters: mainnet / testnet |
| --- | --- | --- | --- | --- | --- |
| Spend Key | `spend` | `testspend` | 96 | 154 | 166 / 170 |
| View Key | `view` | `testview` | 96 | 154 | 165 / 169 |
| Recv Key | `recv` | `testrecv` | 96 | 154 | 165 / 169 |
| Tracking Address | `f` | `t` | 32 | 52 | 60 / 60 |
| Encryption public key component | `e` | `i` | 32 | 52 | 60 / 60 |
| Receiving Address | `f` + `e` | `t` + `i` | 32 + 32 | 52 + 52 | 120 / 120 |

A Tracking Address encodes the spending public key `S`. A Receiving Address
concatenates that complete Tracking Address with the separately encoded
encryption public key `V`, without a separator or an outer checksum:

```text
mainnet: receiving = Bech32f("f", S) ++ Bech32f("e", V)
testnet: receiving = Bech32f("t", S) ++ Bech32f("i", V)
```

Each component has its own prefix, padding and checksum. On either network,
split at exactly 60 characters, then decode both components for the selected
network. The entire Receiving Address MUST be uniformly lowercase or
uppercase; mixed case between components is also invalid. Encoders emit
lowercase. The raw Receiving Address remains `S || V` as defined in FlameKD.

The literal first component can be compared with the Tracking Address shown by
an explorer. This identifies `S`, but does not authenticate the encryption key
`V`; checksums detect errors, not malicious replacement.

## Actor addresses

Actor identity and invocation rules are specified in [Actors](flamevm.md#actors);
binary routing operands are defined in
[Cell-based encoding](encoding.md#actor-addresses-state-and-messages).

### Human-readable actor identifiers

ActorIDs use [Bech32f](bech32f.md) with these network-specific prefixes:

| Mainnet HRP | Testnet HRP | Bytes | Payload symbols | Total characters: mainnet / testnet |
| --- | --- | --- | --- | --- |
| `a` | `ta` | 32 | 52 | 60 / 61 |

Encode only the 32 raw ActorID bytes in their existing order, without routing
tags, length prefixes or invocation attachments. The network prefix does not
change the ID. Any 32-byte value is valid for formatting, with no scalar or
curve-point validation; actor existence is checked separately. Other prefixes
MUST NOT be accepted as actor prefixes.

This formatting vector uses the bytes `00` through `1f`, without claiming an
existing actor:

```text
actor_id = 000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f
mainnet = a1qqqsyqcyq5rqwzqfpg9scrgwpugpzysnzs23v9ccrydpk8qarc0s4qftyf
testnet = ta1qqqsyqcyq5rqwzqfpg9scrgwpugpzysnzs23v9ccrydpk8qarc0sqnka26
```

## Test vectors

These vectors repeat [FlameKD's raw vectors](flamekd.md#raw-test-vectors) as
Bech32f strings. The seed is 64 zero bytes: public test material, **never use it
for funds**. All four path groups below use mainnet HRPs. Paths are descriptive
derivation tests, not the standard wallet account paths. `m/0'` uses
`0x80000000`; its child `16909060` uses `0x01020304` (LE bytes
`04 03 02 01`).

`spend`, `view` and `recv` are extended keys; `receiving` and `tracking`
are their derived payment addresses. Each value is one unbroken ASCII string.

### m

```text
spend = spend1f4uxfwhkp7h8mlqn76hzrj3aj5h0lwq4p5clva42hwpctk44lurhgd4uu472ld8z4smnlvf384armzarejwa05m7fh05akqvw0lk7p6kzaxvr5nfw6jargc8kta5r9ehnghchfqpzfzjl8mmey4j6q03qs4yv900
view = view1tz5t5l8fu7qvx2az3mxuzk2s3690ejm5t9kx720efmls2546gvm8gd4uu472ld8z4smnlvf384armzarejwa05m7fh05akqvw0lk7p6kzaxvr5nfw6jargc8kta5r9ehnghchfqpzfzjl8mmey4j6q03qszcekyq
recv = recv1tz5t5l8fu7qvx2az3mxuzk2s3690ejm5t9kx720efmls2546gvmxvagx0yxfqgdvlscz60n0r58ktp22qcw0s76yk5wnezzamcna5sjkzaxvr5nfw6jargc8kta5r9ehnghchfqpzfzjl8mmey4j6q03qse6tpf4
receiving = f1tz5t5l8fu7qvx2az3mxuzk2s3690ejm5t9kx720efmls2546gvmqxyvkxme1ve6sv7gvjqs6elps95lx78g0vkz55psulpa5fdga8jy9mh38mfpqww34vv
tracking = f1tz5t5l8fu7qvx2az3mxuzk2s3690ejm5t9kx720efmls2546gvmqxyvkxm
```

### m/0

```text
spend = spend1af952hqv53sj4qrkg4mkvtqsdckamfgyhl5dhwuqg6aeypltyspwtgr0q6dy9mq3wvn09u6mwtnse46s9wllzpm003c8h7fmwhkavqyfe38usq3mfzfke5a0jfvague2csknwvryqsz8plcts92sqwjmps39j3vq
view = view1djw5xcjyzwejnh3qp9p8fklwh06r2d0hluc8ysk8lh2jfzsgserwtgr0q6dy9mq3wvn09u6mwtnse46s9wllzpm003c8h7fmwhkavqyfe38usq3mfzfke5a0jfvague2csknwvryqsz8plcts92sqwjmps6acm8k
recv = recv1djw5xcjyzwejnh3qp9p8fklwh06r2d0hluc8ysk8lh2jfzsgsergvs8gnuxtjanh4j2f50x4xugvflvzck70k9k573duhsqs5jyy5vvfe38usq3mfzfke5a0jfvague2csknwvryqsz8plcts92sqwjmpsp4gz22
receiving = f1djw5xcjyzwejnh3qp9p8fklwh06r2d0hluc8ysk8lh2jfzsgserq0u0m93e1seqw38cvh9m80ty5ng7d2dcscn7c93dulvtdfazme0qppfygfgcsy33sak
tracking = f1djw5xcjyzwejnh3qp9p8fklwh06r2d0hluc8ysk8lh2jfzsgserq0u0m93
```

### m/0'

```text
spend = spend16r89eakmeaj7dphe8txsd6nm8gkrh2zufqq7ml5snwd2ag83j5xmra8gtqarg3x8pu3nrwz4v2z3u58l2e828ktzawm30djtk4qgyzrtrwruhtg2ea492mhkaz5na86aqn58tw3a4td08a3h8wfftcp9qgzu607n
view = view13t7k52a3c7av72e72qj44q5levvr34udf4e8hp8685tg9ratwqhtra8gtqarg3x8pu3nrwz4v2z3u58l2e828ktzawm30djtk4qgyzrtrwruhtg2ea492mhkaz5na86aqn58tw3a4td08a3h8wfftcp9qgzrqjl0
recv = recv13t7k52a3c7av72e72qj44q5levvr34udf4e8hp8685tg9ratwqhrf94mj642kjp9ffcx7l5273uxp39df8mnl04kqtvjkg4llkwvguntrwruhtg2ea492mhkaz5na86aqn58tw3a4td08a3h8wfftcp9qgse39qa
receiving = f13t7k52a3c7av72e72qj44q5levvr34udf4e8hp8685tg9ratwqhqr54uq0e1xjtth9424dyz2jnsdalg4arcvrz26j0h87ltvqke9v3tllvuc3eqlccd8t
tracking = f13t7k52a3c7av72e72qj44q5levvr34udf4e8hp8685tg9ratwqhqr54uq0
```

### m/0'/16909060

```text
spend = spend1zl0c8v8tzjaweklwcw3g4ts579sv3e0nykq3hvngrqhxx2f6qqzr8cqgppw9gnjhfdrwmscygjmpd70yjdfjfcdm92jpg65ylhelwrefxas57n4txgxjexhncfxg0se0k8zs3jz3380c0xsrqfv5d8k6pyw4hc9k
view = view13fgep9htef0837vzqjy8m54fzn3jp5mav28dna95nvs6g6wqau2r8cqgppw9gnjhfdrwmscygjmpd70yjdfjfcdm92jpg65ylhelwrefxas57n4txgxjexhncfxg0se0k8zs3jz3380c0xsrqfv5d8k6pynulmhg
recv = recv13fgep9htef0837vzqjy8m54fzn3jp5mav28dna95nvs6g6wqau2t92kz2g96tgk5aeu8j6ul3vx9v003pjt277exj7fx34drs4s2kp3fxas57n4txgxjexhncfxg0se0k8zs3jz3380c0xsrqfv5d8k6pymh0dn0
receiving = f13fgep9htef0837vzqjy8m54fzn3jp5mav28dna95nvs6g6wqau2q8p9qgye1k24vy5st5k3dfmnc094elzcv2c7lzryk4aajd9ujdr268ptq4vrqy3jfmj
tracking = f13fgep9htef0837vzqjy8m54fzn3jp5mav28dna95nvs6g6wqau2q8p9qgy
```

### Root with testnet HRPs

These encode the same raw root tuple with testnet prefixes to test formatting
and network rejection; they do not derive the standard testnet wallet branch.

```text
spend = testspend1f4uxfwhkp7h8mlqn76hzrj3aj5h0lwq4p5clva42hwpctk44lurhgd4uu472ld8z4smnlvf384armzarejwa05m7fh05akqvw0lk7p6kzaxvr5nfw6jargc8kta5r9ehnghchfqpzfzjl8mmey4j6q03qsxee2z5
view = testview1tz5t5l8fu7qvx2az3mxuzk2s3690ejm5t9kx720efmls2546gvm8gd4uu472ld8z4smnlvf384armzarejwa05m7fh05akqvw0lk7p6kzaxvr5nfw6jargc8kta5r9ehnghchfqpzfzjl8mmey4j6q03qs94ys03
recv = testrecv1tz5t5l8fu7qvx2az3mxuzk2s3690ejm5t9kx720efmls2546gvmxvagx0yxfqgdvlscz60n0r58ktp22qcw0s76yk5wnezzamcna5sjkzaxvr5nfw6jargc8kta5r9ehnghchfqpzfzjl8mmey4j6q03qs7hk8zy
receiving = t1tz5t5l8fu7qvx2az3mxuzk2s3690ejm5t9kx720efmls2546gvmqdxl7qzi1ve6sv7gvjqs6elps95lx78g0vkz55psulpa5fdga8jy9mh38mfpq8pwqaf
tracking = t1tz5t5l8fu7qvx2az3mxuzk2s3690ejm5t9kx720efmls2546gvmqdxl7qz
```

