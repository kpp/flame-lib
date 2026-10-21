# One devnet flamed node as an image, its genesis baked in. Build context is
# the repository root. The genesis holder comes from a BIP-39 mnemonic passed
# as a build secret, which reaches no layer of either stage:
#   docker build -f flamed/Dockerfile \
#     --secret id=genesis_mnemonic,src=<file with the phrase> -t flamed .
# Dockerfile.dockerignore next to this file keeps target/ and .git out of the
# context; podman does not read it, so pass it there:
#   podman build --ignorefile flamed/Dockerfile.dockerignore ...
# CI builds and pushes it as ghcr.io/runflame/flamed:<commit> and `:main`
# (.github/workflows/flamed-image.yml).
FROM docker.io/library/rust:1.90-bookworm AS build

# The image already ships 1.90.0. Naming it here overrides
# rust-toolchain.toml, whose clippy/rustfmt/rust-analyzer components a
# release build does not need and rustup would otherwise download.
ENV RUSTUP_TOOLCHAIN=1.90.0

WORKDIR /src
COPY . .

RUN cargo build --release --locked -p flamed --features devnet

# chainparams.toml names one holder by `address`; it is replaced by the first
# receiving address of the mnemonic, so the whole devnet supply is spendable
# by a wallet restored from that phrase. Only the address leaves this step.
# The rewritten file ships beside the genesis: it is the network definition
# every other node of this devnet has to read.
RUN --mount=type=secret,id=genesis_mnemonic,required=true \
    set -eu; \
    params=flamed/configs/chainparams.toml; \
    if [ "$(grep -c '^\[\[genesis\]\]' "$params")" != 1 ] \
        || [ "$(grep -c '^address = ' "$params")" != 1 ]; then \
        echo "$params must hold exactly one [[genesis]] allocation, named by address" >&2; \
        exit 1; \
    fi; \
    holder="$(target/release/flamed address --chainparams "$params" < /run/secrets/genesis_mnemonic)"; \
    mkdir /out; \
    sed "s|^address = .*|address = \"$holder\"|" "$params" > /out/chainparams.toml; \
    target/release/flamed genesis --chainparams /out/chainparams.toml --out /out/genesis.json

FROM docker.io/library/debian:bookworm-slim

LABEL org.opencontainers.image.source=https://github.com/runflame/flame-lib

# The gid is pinned too: `useradd --system` alone would give the group a
# system gid, and a host directory mounted as the volume needs a known owner.
RUN groupadd --system --gid 10001 flamed \
    && useradd --system --uid 10001 --gid 10001 --home-dir /var/lib/flamed flamed \
    && mkdir -p /var/lib/flamed \
    && chown flamed:flamed /var/lib/flamed

COPY --from=build /src/target/release/flamed /usr/local/bin/flamed
COPY flamed/configs/flamed.image.toml /etc/flamed/flamed.toml
COPY --from=build /out/chainparams.toml /out/genesis.json /etc/flamed/

USER flamed
WORKDIR /var/lib/flamed
# blocks.bin, the chain. The genesis is in the image.
VOLUME /var/lib/flamed
EXPOSE 8545

# flamed installs no signal handler, and as PID 1 it is not killed by
# SIGTERM or SIGINT: stopping the container ends in SIGKILL either way.

ENTRYPOINT ["/usr/local/bin/flamed"]
CMD ["run", "--config", "/etc/flamed/flamed.toml"]
