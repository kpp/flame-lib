# Flame Key Derivation

FlameKD defines deterministic hierarchies with separate spending, viewing, and
receiving capabilities. It follows the public-child derivation approach of
[BIP 32](https://github.com/bitcoin/bips/blob/master/bip-0032.mediawiki).

The words MUST, MUST NOT, SHOULD, and MAY specify implementation requirements.
This document specifies derivation and raw key tuples. Human-readable formats
are defined in [Addresses](address.md), using [Bech32f](bech32f.md).

## Group and representations

All group operations use
[ristretto255](https://www.rfc-editor.org/rfc/rfc9496.html#section-4).
Its prime order is:

```
q = 2^252 + 27742317777372353535851937790883648493
```

Lowercase letters denote scalars modulo `q`; uppercase letters denote points.
Scalar addition and subtraction are modulo `q`. Point addition is the group
operation, and `x*G` denotes scalar multiplication by the standard generator.
The canonical compressed encoding of `G` is:

```
e2f2ae0a6abc4e71a884a961c500515f58e30b6aa582dd8db6a65945e08d2d76
```

`enc(x)` for a scalar is its canonical 32-byte little-endian representation,
whose unsigned integer value is less than `q`. Decoding MUST reject values
greater than or equal to `q`; it MUST NOT reduce imported scalars modulo `q`.
`enc(P)` for a point is its canonical 32-byte compressed Ristretto encoding.
Point decoding MUST use Ristretto decoding and reject invalid encodings.

Spending and viewing scalars `s` and `v` MUST be nonzero. Their public points
`S = s*G` and `V = v*G` MUST be nonidentity. Derivation material `t` is a scalar;
zero is permitted. No point `t*G` is used by this specification.

## Key types and capabilities

The tuple order below is normative, including for binary serialization.

| Type | Tuple | Capability |
| --- | --- | --- |
| Spend Key | `(s, v, t)` | Spend, view, and generate addresses; derive normal and hardened children. |
| View Key | `(S, v, t)` | View contents and generate addresses; derive normal children. |
| Recv Key | `(S, V, t)` | Generate receiving addresses and track their activity; derive normal children. |
| Receiving Address | `(S, V)` | Receive encrypted funds at this address. |
| Tracking address | `(S)` | Receive clear funds and identify activity at this spending public key. |

A Recv Key cannot decrypt encrypted contents or authorize spending. A
Tracking address does not identify other addresses in the account. Neither
address type can derive children, because neither contains `t`.

Scalar `s` authorizes spending at its own `S`; scalar `v` enables viewing at its
own `V`. Hierarchical access requires the corresponding extended key. Derivation
material `t` alone is not a tracking capability.

View Keys and Recv Keys cover their own node and descendants reached using only
normal edges. They cannot cross a hardened edge, even when exported at the root.
To share access to a hardened account, derive that account using a Spend Key and
then export its View Key or Recv Key. The standard wallet paths are defined
below; address-discovery lookahead rules are outside this specification.

## Conversions

Conversions are deterministic and do not modify the source key.

| Conversion | Input | Output |
| --- | --- | --- |
| Spend → View | `(s, v, t)` | `(s*G, v, t)` |
| View → Recv | `(S, v, t)` | `(S, v*G, t)` |
| Recv → Receiving Address | `(S, V, t)` | `(S, V)` |
| Receiving Address → Tracking Address | `(S, V)` | `(S)` |

Conversions MAY be composed. For example, a Spend Key can produce every public
address type. The reverse conversions are not provided: removing secrets or
derivation material does not make them recoverable from the resulting value.

## Seed input

The root-derivation input is exactly 64 bytes. An already available binary seed
MUST be consumed directly, without interpreting it as text or hashing it first.
Other binary lengths MUST be rejected.

Mnemonic input MUST first be converted to the standard
[BIP 39 seed](https://github.com/bitcoin/bips/blob/master/bip-0039.mediawiki#from-mnemonic-to-seed).
Use the mnemonic sentence and passphrase normalized with Unicode NFKD and
encoded as UTF-8. An omitted passphrase is the empty string. The conversion is:

```
seed = PBKDF2-HMAC-SHA512(
    password = UTF8(NFKD(mnemonic)),
    salt = UTF8("mnemonic" ++ NFKD(passphrase)),
    iterations = 2048,
    output_length = 64
)
```

Mnemonic import MUST validate the BIP 39 wordlist, length, and checksum, then
serialize the mnemonic words separated by single ASCII spaces before the seed
conversion. Passphrases MUST NOT be trimmed or case-folded. Implementations MUST
NOT use mnemonic entropy bytes in place of the 64-byte BIP 39 seed. The same
mnemonic words and passphrase produce the same 64-byte seed supplied to BIP 32.
FlameKD feeds those bytes directly into its own root transcript; it does not
apply BIP 32's master-key HMAC with key `"Bitcoin seed"`.

## Transcript conventions

`Transcript(domain)` creates a fresh
[Merlin transcript](https://merlin.cool/) using the domain as its initialization
label. Every label in this specification is an exact, case-sensitive ASCII byte
string, without a terminating zero byte.

`P.append(label, bytes)` is Merlin's `append_message` operation. Scalar and point
messages use `enc`; indices use `LE32`, an unsigned four-byte little-endian
encoding. No extra length, type, or tuple encoding is added to these messages;
Merlin supplies its own framing.

`P.challenge_scalar(label)` requests exactly 64 challenge bytes from Merlin,
interprets them as one unsigned 512-bit little-endian integer, and reduces that
integer modulo `q`. It is not a 32-byte challenge or rejection sampling.

All appends and challenges occur sequentially in the stated order on the same
transcript. Each root or child derivation starts a fresh transcript. Transcript
state is not reused across derivations or cloned separately for each challenge.

## Root derivation

Given the 64-byte seed:

```
P = Transcript("FlameKD.from_seed")
P.append("seed", seed)
s = P.challenge_scalar("s")
v = P.challenge_scalar("v")
t = P.challenge_scalar("t")
return SpendKey(s, v, t)
```

If `s = 0` or `v = 0`, root derivation MUST return an error. It MUST NOT retry
with an altered seed, add a counter, or silently choose another transcript.

## Child indices

An index `i` is an unsigned 32-bit integer, `0 <= i < 2^32`.

* `0 <= i < 2^31` selects normal derivation.
* `2^31 <= i < 2^32` selects hardened derivation.

The entire value, including the hardened bit, is appended as `LE32(i)`.
Indices outside the unsigned 32-bit range MUST be rejected. A View Key or
Recv Key MUST reject hardened indices before attempting child derivation.

## Standard wallet derivation paths

Wallets following the Flame account convention MUST use this path:

```
m / 35263' / network' / account' / change / n
```

`m` is the FlameKD root Spend Key derived from the 64-byte seed. Each path
component applies one child derivation to the preceding node. An apostrophe
means hardened derivation: `x'` selects index `2^31 + x`; a component without
an apostrophe selects index `x` directly.

| Component | Value | Derivation |
| --- | --- | --- |
| Flame identifier | `35263`, spelling `FLAME` on a telephone keypad | Hardened |
| `network` | `0` for mainnet, `1` for testnet | Hardened |
| `account` | Account number, starting at `0` | Hardened |
| `change` | `0` for receiving, `1` for change | Normal |
| `n` | Address index within that branch, starting at `0` | Normal |

`account` and `n` MUST be less than `2^31`. The first path component `35263'`
therefore uses the full child index `0x800089bf`. The Flame identifier is a
private convention, not a registered BIP purpose or coin type. The network
component is scoped to Flame.

For account `0`, the receiving paths are:

| Network | Path |
| --- | --- |
| Mainnet | `m/35263'/0'/0'/0/n` |
| Testnet | `m/35263'/1'/0'/0/n` |

Replacing the penultimate component with `1` selects the change branch.
Both Receiving Addresses and Tracking addresses are conversions of the key
at the selected path.

To delegate an account, derive its Spend Key at
`m/35263'/network'/account'`, then export its View Key or Recv Key. Either can
derive `/0/n` and `/1/n`, but cannot derive sibling accounts or cross the
hardened network boundary. To delegate only receiving addresses, export the
Recv Key at `m/35263'/network'/account'/0` instead.

Network and origin path are wallet context; they cannot be recovered from the
raw key tuple. Selecting a different network requires deriving its hardened
network branch.

## Normal derivation

Every normal derivation uses the same parent public values `(S, V, t)`.
A Spend Key computes `S = s*G` and `V = v*G`; a View Key computes `V = v*G`;
a Recv Key already contains both points.

For a normal index `i`, generate the child adjustments and derivation material:

```
P = Transcript("FlameKD.derivation")
P.append("S", enc(S))
P.append("V", enc(V))
P.append("t", enc(t))
P.append("i", LE32(i))
ds = P.challenge_scalar("ds")
dv = P.challenge_scalar("dv")
t_child = P.challenge_scalar("t")
```

Return the corresponding child type:

```
SpendKey(s, v, t) -> SpendKey(s + ds, v + dv, t_child)
ViewKey(S, v, t)  -> ViewKey(S + ds*G, v + dv, t_child)
RecvKey(S, V, t)  -> RecvKey(S + ds*G, V + dv*G, t_child)
```

`ds`, `dv`, and `t_child` may be zero. The resulting spending and viewing keys
must still be valid: `S + ds*G` and `V + dv*G` MUST both be nonidentity.
Equivalently, any available child secret scalar MUST be nonzero.
If either result is invalid, derivation MUST return an error for that index.
It MUST NOT retry, increment the index, or alter the transcript.

For every successful normal derivation, conversion commutes with derivation:

```
view(derive(spend, i)) = derive(view(spend), i)
recv(derive(view, i))  = derive(recv(view), i)
```

Converting these equal children to either address type also gives equal results.
All three parent types can detect either invalid child public point and MUST
agree on whether a normal derivation succeeds.

## Hardened derivation

Only a Spend Key `(s, v, t)` can derive a hardened child. Both secret scalars are
committed, including the secret `v`, not its public point `V`:

```
P = Transcript("FlameKD.hardened")
P.append("s", enc(s))
P.append("v", enc(v))
P.append("t", enc(t))
P.append("i", LE32(i))
ds = P.challenge_scalar("ds")
dv = P.challenge_scalar("dv")
t_child = P.challenge_scalar("t")
return SpendKey(s + ds, v + dv, t_child)
```

If either child secret is zero, derivation MUST return an error for that index,
without retry or index increment. A successful hardened child can subsequently
be converted to its View Key, Recv Key, and addresses.

## Compromise boundaries

All extended keys contain account-linking material and SHOULD be shared only
with parties authorized for their stated capabilities. Neither the Receiving
nor Tracking Address tuple encodes its derivation index or `t`. This does not
promise anonymity against transaction metadata or other external information.

A parent Recv Key can calculate normal-child `ds` and `dv`. Consequently:

- A disclosed normal child viewing scalar reveals parent `v = v_child - dv`.
- A disclosed normal child spending scalar reveals parent `s = s_child - ds`.
- Both disclosures, even from different normal descendants, reveal the parent
  Spend Key. A parent View Key plus a child spending scalar also suffices.

The same equations apply along a known entirely normal path by accumulating
adjustments. Hardened derivation requires both parent secret scalars: recovering
only `s`, while knowing `S`, `V`, and `t`, does not enable hardened derivation.
Use hardened account boundaries when independently delegated subtrees must not
expose their ancestors through these normal-child disclosure relationships.

Token encryption is outside this specification. An encryption protocol SHOULD
use fresh sender randomness per output, validated group inputs, and a
domain-separated KDF bound to the output and recipient; see
[HPKE](https://www.rfc-editor.org/rfc/rfc9180.html#section-4.1).
For selective disclosure, share the final key for the selected encrypted output,
not a viewing scalar or the raw DH point. Raw DH disclosure can also unlock
other outputs when sender points repeat or have known scalar relationships.

## Raw test vectors

These vectors use a binary seed of 64 zero bytes. This is public test material,
not a seed to use for funds. Every value below is raw hexadecimal, split at
32-byte boundaries; concatenate the lines without whitespace. Paths are
descriptive only: `m/0'` uses index `0x80000000`, and `m/0'/16909060` then uses
normal index `0x01020304` (encoded `04 03 02 01`). Corresponding human-readable
vectors are in [Addresses](address.md#test-vectors).

### m

Spend Key (`enc(s) ++ enc(v) ++ enc(t)`):

```
4d7864baf60fae7dfc13f6ae21ca3d952effb8150d31f676aabb8385dab5ff07
7436bce57cafb4e2ac373fb1313d7a3d8ba3cc9dd7d37e4ddf4ed80c73ff6f07
56174cc1d26976a5d1a307b2fb4197379a2f8ba40112452f9f7bc92b2d01f104
```

View Key (`enc(S) ++ enc(v) ++ enc(t)`):

```
58a8ba7ce9e780c32ba28ecdc159508e8afccb74596c6f29f94eff0552ba4336
7436bce57cafb4e2ac373fb1313d7a3d8ba3cc9dd7d37e4ddf4ed80c73ff6f07
56174cc1d26976a5d1a307b2fb4197379a2f8ba40112452f9f7bc92b2d01f104
```

Recv Key (`enc(S) ++ enc(V) ++ enc(t)`):

```
58a8ba7ce9e780c32ba28ecdc159508e8afccb74596c6f29f94eff0552ba4336
667506790c9021acfc302d3e6f1d0f65854a061cf87b44b51d3c885dde27da42
56174cc1d26976a5d1a307b2fb4197379a2f8ba40112452f9f7bc92b2d01f104
```

Receiving Address (`enc(S) ++ enc(V)`):

```
58a8ba7ce9e780c32ba28ecdc159508e8afccb74596c6f29f94eff0552ba4336
667506790c9021acfc302d3e6f1d0f65854a061cf87b44b51d3c885dde27da42
```

Tracking Address (`enc(S)`):

```
58a8ba7ce9e780c32ba28ecdc159508e8afccb74596c6f29f94eff0552ba4336
```

### m/0

Spend Key (`enc(s) ++ enc(v) ++ enc(t)`):

```
ea4b455c0ca4612a80764577662c106e2ddda504bfe8dbbb8046bb9207eb2402
e5a06f069a42ec117326f2f35b72e70cd7502bbff1076f7c707bf93b75edd600
89cc4fc8023b48936cd3af9259d4732ac42d373064040470ff0b8155003a5b0c
```

View Key (`enc(S) ++ enc(v) ++ enc(t)`):

```
6c9d43624413b329de20094274dbeebbf43535f7ff307242c7fdd5248a088646
e5a06f069a42ec117326f2f35b72e70cd7502bbff1076f7c707bf93b75edd600
89cc4fc8023b48936cd3af9259d4732ac42d373064040470ff0b8155003a5b0c
```

Recv Key (`enc(S) ++ enc(V) ++ enc(t)`):

```
6c9d43624413b329de20094274dbeebbf43535f7ff307242c7fdd5248a088646
8640e89f0cb97677ac949a3cd53710c4fd82c5bcfb16d4f45bcbc010a4884a31
89cc4fc8023b48936cd3af9259d4732ac42d373064040470ff0b8155003a5b0c
```

Receiving Address (`enc(S) ++ enc(V)`):

```
6c9d43624413b329de20094274dbeebbf43535f7ff307242c7fdd5248a088646
8640e89f0cb97677ac949a3cd53710c4fd82c5bcfb16d4f45bcbc010a4884a31
```

Tracking Address (`enc(S)`):

```
6c9d43624413b329de20094274dbeebbf43535f7ff307242c7fdd5248a088646
```

### m/0'

Spend Key (`enc(s) ++ enc(v) ++ enc(t)`):

```
d0ce5cf6dbcf65e686f93acd06ea7b3a2c3ba85c4801edfe909b9aaea0f1950d
b1f4e8583a3444c70f2331b85562851e50ff564ea3d962ebb717b64bb5408208
6b1b87cbad0acf6a556ef6e8a93e9f5d04e875ba3daadaf3f6373b9295e02502
```

View Key (`enc(S) ++ enc(v) ++ enc(t)`):

```
8afd6a2bb1c7bacf2b3e50255a829fcb1838d78d4d727b84fa3d16828fab702e
b1f4e8583a3444c70f2331b85562851e50ff564ea3d962ebb717b64bb5408208
6b1b87cbad0acf6a556ef6e8a93e9f5d04e875ba3daadaf3f6373b9295e02502
```

Recv Key (`enc(S) ++ enc(V) ++ enc(t)`):

```
8afd6a2bb1c7bacf2b3e50255a829fcb1838d78d4d727b84fa3d16828fab702e
3496bb96aaab48254a706f7e8af47860c4ad49f73fbeb602d92b22bffd9cc472
6b1b87cbad0acf6a556ef6e8a93e9f5d04e875ba3daadaf3f6373b9295e02502
```

Receiving Address (`enc(S) ++ enc(V)`):

```
8afd6a2bb1c7bacf2b3e50255a829fcb1838d78d4d727b84fa3d16828fab702e
3496bb96aaab48254a706f7e8af47860c4ad49f73fbeb602d92b22bffd9cc472
```

Tracking Address (`enc(S)`):

```
8afd6a2bb1c7bacf2b3e50255a829fcb1838d78d4d727b84fa3d16828fab702e
```

### m/0'/16909060

Spend Key (`enc(s) ++ enc(v) ++ enc(t)`):

```
17df83b0eb14baecdbeec3a28aae14f160c8e5f325811bb268182e63293a0004
33e008085c544e574b46edc30444b616f9e4935324e1bb2aa4146a84fdf3f70f
2937614f4eab320d2c9af3c24c87c32fb1c508c85189df879a030259469eda09
```

View Key (`enc(S) ++ enc(v) ++ enc(t)`):

```
8a519096ebca5e78f98204887dd2a914e320d37d628ed9f4b49b21a469c0ef14
33e008085c544e574b46edc30444b616f9e4935324e1bb2aa4146a84fdf3f70f
2937614f4eab320d2c9af3c24c87c32fb1c508c85189df879a030259469eda09
```

Recv Key (`enc(S) ++ enc(V) ++ enc(t)`):

```
8a519096ebca5e78f98204887dd2a914e320d37d628ed9f4b49b21a469c0ef14
b2aac2520ba5a2d4ee78796b9f8b0c563df10c96af7b26979268d5a38560ab06
2937614f4eab320d2c9af3c24c87c32fb1c508c85189df879a030259469eda09
```

Receiving Address (`enc(S) ++ enc(V)`):

```
8a519096ebca5e78f98204887dd2a914e320d37d628ed9f4b49b21a469c0ef14
b2aac2520ba5a2d4ee78796b9f8b0c563df10c96af7b26979268d5a38560ab06
```

Tracking Address (`enc(S)`):

```
8a519096ebca5e78f98204887dd2a914e320d37d628ed9f4b49b21a469c0ef14
```

### BIP 39 compatibility

The English mnemonic `abandon` repeated eleven times followed by `about`, with
passphrase `TREZOR`, MUST produce the following 64-byte intermediate seed:

```
c55257c360c07c72029aebc1b53c05ed0362ada38ead3e3e9efa3708e5349553
1f09a6987599d18264c1e1c92f2cf141630c7a3c4ab7c81b2f001698e7463b04
```

Concatenate these hex lines. This is the BIP 39 reference vector, not the
all-zero seed used for the FlameKD vectors above.
