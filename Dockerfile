# syntax=docker/dockerfile:1
FROM rust:1.98.1-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY src/ src/
COPY migrations/ migrations/
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --locked --release && cp target/release/tg-bot-predlozhka /usr/local/bin/bot

FROM build AS test
COPY tests/ tests/
COPY messages.json ./
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo test --locked

# No HEALTHCHECK on purpose: a periodic exec is what made the Python bot's
# container burn 1.4 % of a core. Docker's restart policy covers crashes.
FROM gcr.io/distroless/cc-debian12:nonroot
WORKDIR /app
COPY --from=build /usr/local/bin/bot /app/bot
COPY messages.json /app/messages.json
ENV DATA_DIR=/app/data MESSAGES_PATH=/app/messages.json RUST_LOG=info NO_COLOR=1
USER 65532:65532
ENTRYPOINT ["/app/bot"]
