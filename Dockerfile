# syntax=docker/dockerfile:1.7
FROM rust:1-bookworm AS builder

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY contracts ./contracts
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --release --locked && \
    cp /build/target/release/aomori /tmp/aomori

FROM debian:bookworm-slim AS runtime

RUN apt-get update && \
    apt-get install --no-install-recommends -y ca-certificates curl && \
    rm -rf /var/lib/apt/lists/* && \
    groupadd --gid 10001 aomori && \
    useradd --uid 10001 --gid aomori --home-dir /nonexistent --shell /usr/sbin/nologin aomori && \
    install -d -o aomori -g aomori /data

COPY --from=builder /tmp/aomori /usr/local/bin/aomori

USER 10001:10001
WORKDIR /data
EXPOSE 8091
VOLUME ["/data"]

HEALTHCHECK --interval=15s --timeout=3s --start-period=10s --retries=3 \
    CMD ["curl", "--fail", "--silent", "--show-error", "http://127.0.0.1:8091/ready"]

ENTRYPOINT ["/usr/local/bin/aomori"]
CMD ["--listen", "0.0.0.0:8091", "--data-dir", "/data", "--demo"]
