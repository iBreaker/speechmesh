FROM rust:1.86-bookworm AS builder
WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY core ./core
COPY asr ./asr
COPY tts ./tts
COPY transport ./transport
COPY device ./device
COPY bridges ./bridges
COPY app ./app
COPY speechmeshd ./speechmeshd
COPY sdks/rust ./sdks/rust

RUN cargo build --locked -p speechmeshd --release --bin speechmeshd

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --uid 10001 speechmesh

COPY --from=builder /app/target/release/speechmeshd /usr/local/bin/speechmeshd

USER speechmesh
EXPOSE 8765
ENTRYPOINT ["/usr/local/bin/speechmeshd"]
