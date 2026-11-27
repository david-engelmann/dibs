FROM rust:1.91.1-bookworm AS builder
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY bindings ./bindings
COPY src ./src
RUN cargo build --release --locked --bin dibs \
    && strip /src/target/release/dibs

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 65532 dibs \
    && useradd --uid 65532 --gid 65532 --create-home --shell /usr/sbin/nologin dibs \
    && mkdir -p /work \
    && chown dibs:dibs /work
COPY --from=builder /src/target/release/dibs /usr/local/bin/dibs
USER 65532:65532
WORKDIR /work
ENTRYPOINT ["dibs"]
