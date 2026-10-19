# Bech32f

Bech32f is Flame's byte-to-text checksum encoding. Key and address types, networks,
HRPs and exact lengths are defined separately in [Address formats](address.md).
Key derivation is specified in [FlameKD](flamekd.md).

The words MUST, MUST NOT, SHOULD and MAY specify implementation requirements.

## Format

Bech32f uses the alphabet, HRP expansion, six-symbol checksum and polymod
algorithm of [Bech32m](https://github.com/bitcoin/bips/blob/master/bip-0350.mediawiki),
with checksum constant `0x2bc830a3`. Original Bech32 checksums with constant
`1` MUST NOT be accepted.

Unlike BIP 173/350, there is no universal 90-character limit. Each application
MUST specify the accepted HRPs and exact payload lengths, and bound input before
decoding. This length override has precedent in
[ZIP 316](https://zips.z.cash/zip-0316).

```text
text = HRP ++ "1" ++ payload_symbols ++ checksum_symbols
alphabet = "qpzry9x8gf2tvdw0s3jn54khce6mua7l"
```

The alphabet is ordered by five-bit value, from 0 through 31. The HRP contains
1–83 printable ASCII characters (values 33–126); the separator is the last
`1` in the text. No version or type symbol is inserted into the payload:
the application identifies the expected type through the HRP.

Encode each Bech32f component's complete payload in one 8-to-5-bit conversion,
reading bytes most-significant bit first. Zero-pad only the final five-bit
group. Do not separately pad tuple elements within a component. An application
may concatenate independently encoded components, as specified for
[Receiving Addresses](address.md#keys-and-payment-addresses); each then has its
own HRP and checksum, and the application specifies component boundaries.

## Checksum

For the lowercase HRP, `hrp_expand` is the upper five bits of each character,
a zero symbol, then the lower five bits of each character.

```text
hrp_expand(hrp) = [ord(c) >> 5 for c in hrp] ++ [0]
               ++ [ord(c) & 31 for c in hrp]

GEN = [0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3]

polymod(symbols):
    checksum = 1
    for symbol in symbols:
        top = checksum >> 25
        checksum = ((checksum & 0x1ffffff) << 5) XOR symbol
        for i in 0..5:
            if (top >> i) & 1:
                checksum = checksum XOR GEN[i]
    return checksum

checksum = polymod(hrp_expand(hrp) ++ payload_symbols ++ [0,0,0,0,0,0])
           XOR 0x2bc830a3
checksum_symbols = [(checksum >> (5 * (5-i))) & 31 for i in 0..6]
```

Ranges above exclude their upper bound. Verification MUST produce
`0x2bc830a3` from `polymod(hrp_expand(hrp) ++ payload_symbols ++ checksum_symbols)`.
The checksum binds the HRP; it MUST be recomputed when changing a prefix.

## Canonical decoding

Encoders MUST emit lowercase ASCII. Decoders MUST accept entirely lowercase or
entirely uppercase text, reject mixed case, and normalize accepted uppercase
input to lowercase before HRP interpretation and checksum verification.

Decoders MUST reject non-ASCII text, whitespace, invalid HRP characters,
unknown/application-inappropriate HRPs, invalid alphabet symbols, invalid
checksums and incorrect application lengths.

After removing the checksum, convert 5-to-8 bits without adding padding.
Remaining bits MUST be fewer than five and all zero. The decoded byte count MUST
match the application's expected length. Typed payload validation follows this
check; checksum validity does not establish point/scalar validity.

## Error detection

For a fixed valid HRP and fixed length, the checksum detects every pattern of
up to three substituted payload/checksum symbols at Flame's current key/address
lengths. It also detects up to four substitutions when they fit within an
89-symbol window. It does not guarantee detection of every four-symbol
substitution across longer strings. These are changed-symbol guarantees, not
guarantees about insertions, deletions, HRP changes or case normalization.

See [BIP 173 checksum properties](https://github.com/bitcoin/bips/blob/master/bip-0173.mediawiki#checksum-design)
and [BIP 350 checksum analysis](https://github.com/bitcoin/bips/blob/master/bip-0350.mediawiki#appendix-checksum-design--properties).
The three-substitution guarantee extends to 1023 symbols, as described in
[Bitcoin Core's polymod implementation](https://github.com/bitcoin/bitcoin/blob/master/src/bech32.cpp).

For fixed HRP and payload, a uniformly random six-symbol checksum is accepted
with probability `2^-30`. The checksum is not authentication: anyone can
recompute it for modified data.

The detection bounds guarantee unique correction of one substituted symbol
with fixed HRP. They do not guarantee unique correction of two across longer
strings. Decoders MUST NOT automatically correct invalid input. A user interface
MAY suggest possible error positions, but MUST NOT silently substitute a guessed
key or claim that the number of errors is known.

Typed, implementation-checked test vectors are in [Address formats](address.md#test-vectors).

