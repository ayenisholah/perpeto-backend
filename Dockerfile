# syntax=docker/dockerfile:1.10
FROM rust:1.97.0-slim-bookworm AS builder
WORKDIR /workspace
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY bins ./bins
COPY migrations ./migrations
RUN cargo build --locked --release --workspace

FROM debian:bookworm-slim AS runtime
RUN useradd --create-home --uid 10001 perpeto
COPY --from=builder /workspace/target/release/funding-arb-api /usr/local/bin/
COPY --from=builder /workspace/target/release/funding-arb-engine /usr/local/bin/
COPY --from=builder /workspace/target/release/funding-arb-reconciler /usr/local/bin/
COPY --from=builder /workspace/target/release/funding-arb-preflight /usr/local/bin/
COPY --from=builder /workspace/target/release/funding-arb-rewrap /usr/local/bin/
COPY --from=builder /workspace/target/release/funding-arb-connector-evidence /usr/local/bin/
COPY --from=builder /workspace/target/release/funding-arb-worker /usr/local/bin/
COPY --from=builder /workspace/target/release/funding-arb-signer /usr/local/bin/
COPY --from=builder /workspace/target/release/funding-arb-migrate /usr/local/bin/
USER 10001:10001
ENV PERPETO_MODE=PAPER
ENTRYPOINT ["funding-arb-api"]
