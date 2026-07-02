FROM rust:1-slim AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

FROM debian:stable-slim
COPY --from=builder /app/target/release/timecoin-node /usr/local/bin/timecoin-node
VOLUME /data
# 9000: libp2p (TCP + QUIC), 3000: HTTP API
EXPOSE 9000/tcp 9000/udp 3000/tcp
ENTRYPOINT ["timecoin-node"]
CMD ["run", "--data-dir", "/data", "--port", "9000", "--api", "0.0.0.0:3000"]
