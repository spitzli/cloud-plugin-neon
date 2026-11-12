# Context is this plugin repository. Commit regenerated Cargo.lock before building.
# Runtime configuration, issuer keys, mTLS identity and BYOK credentials are mounted by Core.
FROM docker.io/library/rust:1.98.1-slim-trixie@sha256:4cd829461bd5c4d511c32e269da9cb8929223b666519d8004e35fc8d1d771ab7 AS source
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler ca-certificates pkg-config && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
# Shared only among reviewed Spitzli platform sources; never customer build code.
FROM source AS check
RUN --mount=type=cache,id=spitzli-native-sdk-8019f3df67dba3fed3083f22635e4725028c1b5e-registry,target=/usr/local/cargo/registry,sharing=locked --mount=type=cache,id=spitzli-native-sdk-8019f3df67dba3fed3083f22635e4725028c1b5e-git,target=/usr/local/cargo/git,sharing=locked --mount=type=cache,id=spitzli-native-sdk-8019f3df67dba3fed3083f22635e4725028c1b5e-target,target=/src/target,sharing=locked cargo check --locked --all-targets --jobs 3 && cargo test --locked --all-targets --jobs 3

FROM source AS build
RUN --mount=type=cache,id=spitzli-native-sdk-8019f3df67dba3fed3083f22635e4725028c1b5e-registry,target=/usr/local/cargo/registry,sharing=locked --mount=type=cache,id=spitzli-native-sdk-8019f3df67dba3fed3083f22635e4725028c1b5e-git,target=/usr/local/cargo/git,sharing=locked --mount=type=cache,id=spitzli-native-sdk-8019f3df67dba3fed3083f22635e4725028c1b5e-target,target=/src/target,sharing=locked cargo build --locked --release --bins --jobs 3 && mkdir /out && install -m 0555 /src/target/release/spitzli-neon /out/spitzli-neon

FROM docker.io/library/debian:trixie-slim@sha256:f6b08b09a05725f9431502d3b5714f5c4576e82c1a84fa1debe0c24a7e0c3ec4
COPY --from=build /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY LICENSE COMMERCIAL-LICENSING.md NOTICE /usr/share/licenses/spitzli-cloud/
COPY --from=build --chown=65532:65532 --chmod=0555 /out/spitzli-neon /usr/local/bin/spitzli-neon
ENV HOME=/tmp
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/spitzli-neon"]
