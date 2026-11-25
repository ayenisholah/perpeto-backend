# syntax=docker/dockerfile:1.10
FROM rust:1.97.0-slim-bookworm AS builder
# `webauthn-rs` pulls in `openssl-sys`, which links the system OpenSSL and needs
# `pkg-config` to locate it. The slim Rust image ships neither, unlike GitHub's
# runners, so the workspace only builds here once both are installed.
RUN apt-get update \
 && apt-get install -y --no-install-recommends pkg-config libssl-dev \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /workspace
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY bins ./bins
COPY migrations ./migrations
RUN cargo build --locked --release --workspace

FROM debian:bookworm-slim AS runtime
# OpenSSL is linked dynamically (`libssl.so.3`, `libcrypto.so.3`), so the runtime
# image needs the library even though it does not compile anything; without it
# every binary fails to start. `ca-certificates` is required to verify venue TLS.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates libssl3 \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --create-home --uid 10001 perpeto
COPY --from=builder /workspace/target/release/funding-arb-api /usr/local/bin/
COPY --from=builder /workspace/target/release/funding-arb-engine /usr/local/bin/
COPY --from=builder /workspace/target/release/funding-arb-reconciler /usr/local/bin/
COPY --from=builder /workspace/target/release/funding-arb-preflight /usr/local/bin/
COPY --from=builder /workspace/target/release/funding-arb-rewrap /usr/local/bin/
COPY --from=builder /workspace/target/release/funding-arb-connector-evidence /usr/local/bin/
COPY --from=builder /workspace/target/release/funding-arb-worker /usr/local/bin/
COPY --from=builder /workspace/target/release/funding-arb-signer /usr/local/bin/
COPY --from=builder /workspace/target/release/funding-arb-migrate /usr/local/bin/
COPY --from=builder /workspace/target/release/funding-arb-venue-admin /usr/local/bin/
COPY --from=builder /workspace/target/release/funding-arb-sandbox /usr/local/bin/
USER 10001:10001
ENV PERPETO_MODE=PAPER
ENTRYPOINT ["funding-arb-api"]
