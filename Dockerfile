# syntax=docker/dockerfile:1
#
# necropsy web server image. Packs are baked in, so each image is a complete,
# versioned release: the blue and green containers of a deploy differ only in
# which image they run. Settings come from the environment at runtime and the
# database lives on a volume mounted at /data.

ARG RUST_VERSION=1.100

FROM rust:${RUST_VERSION}-bookworm AS build
WORKDIR /src

# Build dependencies first so a source-only change reuses their layer.
COPY Cargo.toml Cargo.lock ./
COPY crates/necropsy-pack/Cargo.toml crates/necropsy-pack/Cargo.toml
COPY crates/necropsy-web/Cargo.toml crates/necropsy-web/Cargo.toml
RUN mkdir -p crates/necropsy-pack/src/bin crates/necropsy-web/src \
 && echo '' > crates/necropsy-pack/src/lib.rs \
 && echo 'fn main() {}' > crates/necropsy-pack/src/bin/pack-tool.rs \
 && echo 'fn main() {}' > crates/necropsy-web/src/main.rs \
 && cargo build --release --locked -p necropsy-web \
 && rm -rf crates/*/src target/release/necropsy-web target/release/deps/necropsy* target/release/.fingerprint/necropsy*

COPY crates crates
COPY docs docs
RUN cargo build --release --locked -p necropsy-web \
 && mkdir /data

# Distroless: no shell or package manager in the final image. The base image
# already provides the unprivileged "nonroot" user (uid 65532).
FROM gcr.io/distroless/cc-debian12:nonroot

ARG GIT_SHA=unknown
LABEL org.opencontainers.image.title="necropsy" \
      org.opencontainers.image.source="https://github.com/urashyl/necropsy" \
      org.opencontainers.image.licenses="AGPL-3.0-or-later" \
      org.opencontainers.image.revision="${GIT_SHA}"

COPY --from=build /src/target/release/necropsy-web /app/necropsy-web
COPY packs /app/packs
# Created here so a new volume mounted at /data starts out writable by nonroot.
COPY --from=build --chown=65532:65532 /data /data

ENV NECROPSY_ADDR=0.0.0.0:8080 \
    NECROPSY_PACKS=/app/packs \
    NECROPSY_DB=/data/necropsy.db

EXPOSE 8080
VOLUME ["/data"]
WORKDIR /data

# The server shuts down gracefully on SIGINT (docker stop sends SIGTERM by default).
STOPSIGNAL SIGINT

HEALTHCHECK --interval=5s --timeout=4s --start-period=10s --retries=3 \
  CMD ["/app/necropsy-web", "healthcheck"]

ENTRYPOINT ["/app/necropsy-web"]
