# Address formats

This document specifies Flame's binary actor addresses and human-readable actor,
key and payment-address formats. [Bech32f](bech32f.md) specifies the text codec;
[FlameKD](flamekd.md) specifies key derivation and raw cryptographic tuples.
[Cell-based encodings](encoding.md) specify the common binary encoding rules.

The words MUST, MUST NOT, SHOULD and MAY specify implementation requirements.

## Keys and payment addresses

`enc(s)` is a canonical 32-byte little-endian scalar; `enc(S)` is a canonical
32-byte compressed Ristretto point. Tuple order and cryptographic validation
are defined in [FlameKD](flamekd.md#group-and-representations).

| Type | Mainnet HRP | Testnet HRP | Raw bytes | Bytes | Payload symbols | Total characters: mainnet / testnet |
| --- | --- | --- | --- | --- | --- | --- |
| Spend Key | `spend` | `testspend` | `enc(s) ++ enc(v) ++ enc(t)` | 96 | 154 | 166 / 170 |
| View Key | `view` | `testview` | `enc(S) ++ enc(v) ++ enc(t)` | 96 | 154 | 165 / 169 |
| Recv Key | `recv` | `testrecv` | `enc(S) ++ enc(V) ++ enc(t)` | 96 | 154 | 165 / 169 |
| Receiving Address | `f` | `tf` | `enc(S) ++ enc(V)` | 64 | 103 | 111 / 112 |
| Tracking Address | `c` | `tc` | `enc(S)` | 32 | 52 | 60 / 61 |

`++` denotes concatenation. Encode the entire tuple in one 8-to-5-bit
conversion. Zero padding is two bits for 96 bytes, three for 64 bytes and four
for 32 bytes; do not independently pad its 32-byte elements.

Decoders MUST select the expected type/network HRP, enforce the exact lengths
above, and validate each scalar and point after checksum and padding checks.
Spending/viewing scalars MUST be nonzero and spending/viewing points MUST be
nonidentity. Derivation scalar `t` may be zero. No trailing bytes or fields
are permitted. A valid checksum is not cryptographic validation.

Testnet uses `"test" ++ mainnet_HRP` for keys and `"t" ++ mainnet_HRP` for
payment addresses. Prefixes are case-sensitive after Bech32f case normalization.
Changing a prefix changes neither the tuple nor its derivation path; the
checksum must nevertheless be recomputed.

Wallets MUST use the HRP matching their selected network and
[standard wallet derivation path](flamekd.md#standard-wallet-derivation-paths).
Producing the corresponding key on another network requires deriving that
network's hardened branch, not merely changing the prefix. Network and origin
path are wallet context, not recoverable from the raw tuple.

Extended keys contain sensitive or account-linking material. Receiving and
tracking addresses omit derivation material and path/index metadata. Their
different lengths and HRPs MUST NOT be treated as interchangeable encodings.

## Actor addresses

The following CTL is the canonical actor-address schema throughout FlameVM.
Actor identity is a raw 32-byte hash, not a public key or a tagged sum:

```ctl
type ActorID = [U8; 32];

enum ActorAddress: U8 {
    Init { ctor: ^Cell } = 0,
    Address { addr: ActorID } = 1,
    Restore { addr: ActorID, state: ^Cell } = 2,
}
```

`ActorID` has exactly 32 payload bytes in their existing order: no tag, length
prefix or endian reversal. The actor's immutable ID is the constructor Cell's
factual ID (highest-level hash). It is not recomputed from current code/data,
and `Restore` cannot choose a new identity.

| Variant | Address Cell payload | Address Cell references |
| --- | --- | --- |
| `Init` | `00` | `[ctor]` |
| `Address` | `01 ++ addr:32` | None |
| `Restore` | `02 ++ addr:32` | `[state]` |

All other tags, incorrect payload lengths and extra references MUST be rejected.
`Init` carries constructor availability; `Address` carries only the ID;
`Restore` supplies availability authenticated against the registered actor
state commitment. Absence of supplied data never means empty logical data.

An actor's content is one root Cell with no payload and exactly two references,
ordered `[code, data]`. Freezing prunes that **whole root**, not code and data
independently. The `state` reference in `Restore` targets that complete root;
its descendants may remain pruned. Reading an unavailable branch fails the
internal transaction rather than returning a default or authorizing guessed
data. Detailed compression and lifecycle requirements are in
[Compression](compression.md).

The Rust actor codecs are generated from `flamevm/actor.ctl`. This replaces the
previous Hash/Constructor sum and reverses its two tags; that earlier wire form
is not accepted. VM `send` and `call` take the native address Cell itself, not
bare identity bytes or a serialized transport envelope.

### Human-readable actor identifiers

ActorIDs use [Bech32f](bech32f.md) with these network-specific prefixes:

| Mainnet HRP | Testnet HRP | Bytes | Payload symbols | Total characters: mainnet / testnet |
| --- | --- | --- | --- | --- |
| `a` | `ta` | 32 | 52 | 60 / 61 |

Encode the 32 raw ActorID bytes in their existing order with one 8-to-5-bit
conversion and four final zero padding bits. The payload has no ActorAddress
tag, length prefix or endian reversal. Constructor/restoration Cell graphs
remain invocation attachments.

Encoders MUST emit lowercase. Decoders MUST apply Bech32f's checksum, case and
padding rules, enforce the exact length and selected network's HRP, and reject
the other network. The prefix identifies the network without changing the ID.
ActorIDs are hashes: any 32-byte value is valid for formatting, with no scalar
or curve-point validation. Actor existence is checked separately. `c` / `tc`
identify Tracking Addresses and MUST NOT be accepted as actor prefixes.

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
receiving = f1tz5t5l8fu7qvx2az3mxuzk2s3690ejm5t9kx720efmls2546gvmxvagx0yxfqgdvlscz60n0r58ktp22qcw0s76yk5wnezzamcna5ssfgcs3c
tracking = c1tz5t5l8fu7qvx2az3mxuzk2s3690ejm5t9kx720efmls2546gvmq72w85x
```

### m/0

```text
spend = spend1af952hqv53sj4qrkg4mkvtqsdckamfgyhl5dhwuqg6aeypltyspwtgr0q6dy9mq3wvn09u6mwtnse46s9wllzpm003c8h7fmwhkavqyfe38usq3mfzfke5a0jfvague2csknwvryqsz8plcts92sqwjmps39j3vq
view = view1djw5xcjyzwejnh3qp9p8fklwh06r2d0hluc8ysk8lh2jfzsgserwtgr0q6dy9mq3wvn09u6mwtnse46s9wllzpm003c8h7fmwhkavqyfe38usq3mfzfke5a0jfvague2csknwvryqsz8plcts92sqwjmps6acm8k
recv = recv1djw5xcjyzwejnh3qp9p8fklwh06r2d0hluc8ysk8lh2jfzsgsergvs8gnuxtjanh4j2f50x4xugvflvzck70k9k573duhsqs5jyy5vvfe38usq3mfzfke5a0jfvague2csknwvryqsz8plcts92sqwjmpsp4gz22
receiving = f1djw5xcjyzwejnh3qp9p8fklwh06r2d0hluc8ysk8lh2jfzsgsergvs8gnuxtjanh4j2f50x4xugvflvzck70k9k573duhsqs5jyy5vgdmtj30
tracking = c1djw5xcjyzwejnh3qp9p8fklwh06r2d0hluc8ysk8lh2jfzsgserqhjd2hv
```

### m/0'

```text
spend = spend16r89eakmeaj7dphe8txsd6nm8gkrh2zufqq7ml5snwd2ag83j5xmra8gtqarg3x8pu3nrwz4v2z3u58l2e828ktzawm30djtk4qgyzrtrwruhtg2ea492mhkaz5na86aqn58tw3a4td08a3h8wfftcp9qgzu607n
view = view13t7k52a3c7av72e72qj44q5levvr34udf4e8hp8685tg9ratwqhtra8gtqarg3x8pu3nrwz4v2z3u58l2e828ktzawm30djtk4qgyzrtrwruhtg2ea492mhkaz5na86aqn58tw3a4td08a3h8wfftcp9qgzrqjl0
recv = recv13t7k52a3c7av72e72qj44q5levvr34udf4e8hp8685tg9ratwqhrf94mj642kjp9ffcx7l5273uxp39df8mnl04kqtvjkg4llkwvguntrwruhtg2ea492mhkaz5na86aqn58tw3a4td08a3h8wfftcp9qgse39qa
receiving = f13t7k52a3c7av72e72qj44q5levvr34udf4e8hp8685tg9ratwqhrf94mj642kjp9ffcx7l5273uxp39df8mnl04kqtvjkg4llkwvgushdv07q
tracking = c13t7k52a3c7av72e72qj44q5levvr34udf4e8hp8685tg9ratwqhqm6hdjj
```

### m/0'/16909060

```text
spend = spend1zl0c8v8tzjaweklwcw3g4ts579sv3e0nykq3hvngrqhxx2f6qqzr8cqgppw9gnjhfdrwmscygjmpd70yjdfjfcdm92jpg65ylhelwrefxas57n4txgxjexhncfxg0se0k8zs3jz3380c0xsrqfv5d8k6pyw4hc9k
view = view13fgep9htef0837vzqjy8m54fzn3jp5mav28dna95nvs6g6wqau2r8cqgppw9gnjhfdrwmscygjmpd70yjdfjfcdm92jpg65ylhelwrefxas57n4txgxjexhncfxg0se0k8zs3jz3380c0xsrqfv5d8k6pynulmhg
recv = recv13fgep9htef0837vzqjy8m54fzn3jp5mav28dna95nvs6g6wqau2t92kz2g96tgk5aeu8j6ul3vx9v003pjt277exj7fx34drs4s2kp3fxas57n4txgxjexhncfxg0se0k8zs3jz3380c0xsrqfv5d8k6pymh0dn0
receiving = f13fgep9htef0837vzqjy8m54fzn3jp5mav28dna95nvs6g6wqau2t92kz2g96tgk5aeu8j6ul3vx9v003pjt277exj7fx34drs4s2kps2e0jrr
tracking = c13fgep9htef0837vzqjy8m54fzn3jp5mav28dna95nvs6g6wqau2ql0836e
```

### Root with testnet HRPs

These encode the same raw root tuple with testnet prefixes to test formatting
and network rejection; they do not derive the standard testnet wallet branch.

```text
spend = testspend1f4uxfwhkp7h8mlqn76hzrj3aj5h0lwq4p5clva42hwpctk44lurhgd4uu472ld8z4smnlvf384armzarejwa05m7fh05akqvw0lk7p6kzaxvr5nfw6jargc8kta5r9ehnghchfqpzfzjl8mmey4j6q03qsxee2z5
view = testview1tz5t5l8fu7qvx2az3mxuzk2s3690ejm5t9kx720efmls2546gvm8gd4uu472ld8z4smnlvf384armzarejwa05m7fh05akqvw0lk7p6kzaxvr5nfw6jargc8kta5r9ehnghchfqpzfzjl8mmey4j6q03qs94ys03
recv = testrecv1tz5t5l8fu7qvx2az3mxuzk2s3690ejm5t9kx720efmls2546gvmxvagx0yxfqgdvlscz60n0r58ktp22qcw0s76yk5wnezzamcna5sjkzaxvr5nfw6jargc8kta5r9ehnghchfqpzfzjl8mmey4j6q03qs7hk8zy
receiving = tf1tz5t5l8fu7qvx2az3mxuzk2s3690ejm5t9kx720efmls2546gvmxvagx0yxfqgdvlscz60n0r58ktp22qcw0s76yk5wnezzamcna5sstwlmnd
tracking = tc1tz5t5l8fu7qvx2az3mxuzk2s3690ejm5t9kx720efmls2546gvmqte3364
```

