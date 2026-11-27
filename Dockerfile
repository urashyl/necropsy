# syntax=docker/dockerfile:1
#
# necropsy web server image. Packs are baked in, so each image is a complete,
# versioned release: the blue and green containers of a deploy differ only in
# which image they run. Settings come from the environment at runtime and the
# database lives on a volume mounted at /data.

ARG RUST_VERSION=1.100

FROM rust:${RUST_VERSION}-bookworm AS build
WORKDIR /src

# A plain build of the real sources, with two cache mounts that live in BuildKit's
# cache on the machine doing the build: the downloaded crates, and the compiled
# `target` directory. The second is what keeps a deploy from compiling every
# dependency again: only this repository's own crates are rebuilt (a fresh checkout
# gives every source file a new time, so Cargo does consider them changed; registry
# crates are keyed by version, not by time, so they are not). Cargo decides what is
# stale for itself, so there are no stub sources to get wrong. A cache mount is not
# part of the image, so the binary is copied out in the same step. If the cache is
# ever cleared, the next build is simply a full one.
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY docs docs
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo build --release --locked -p necropsy-web \
 && install -D target/release/necropsy-web /out/necropsy-web \
 && mkdir /data

# The in-browser toolkit's third-party builds (Wiregasm, Pyodide, CyberChef),
# downloaded and checked against the SHA-256 sums in toolkit/sources.txt. A stage
# of its own, so it is cached until sources.txt or fetch.sh change.
FROM debian:bookworm-slim AS toolkit
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl unzip gzip \
 && rm -rf /var/lib/apt/lists/*
COPY toolkit/sources.txt toolkit/fetch.sh /src/toolkit/
RUN /src/toolkit/fetch.sh /out/toolkit

# Distroless: no shell or package manager in the final image. The base image
# already provides the unprivileged "nonroot" user (uid 65532).
FROM gcr.io/distroless/cc-debian12:nonroot

ARG GIT_SHA=unknown
LABEL org.opencontainers.image.title="necropsy" \
      org.opencontainers.image.source="https://github.com/urashyl/necropsy" \
      org.opencontainers.image.licenses="AGPL-3.0-or-later" \
      org.opencontainers.image.revision="${GIT_SHA}"

COPY --from=build /out/necropsy-web /app/necropsy-web
COPY packs /app/packs
COPY learn /app/learn
COPY --from=toolkit /out/toolkit /app/toolkit
# Created here so a new volume mounted at /data starts out writable by nonroot.
COPY --from=build --chown=65532:65532 /data /data

ENV NECROPSY_ADDR=0.0.0.0:8080 \
    NECROPSY_PACKS=/app/packs \
    NECROPSY_LEARN=/app/learn \
    NECROPSY_TOOLKIT=/app/toolkit \
    NECROPSY_DB=/data/necropsy.db

EXPOSE 8080
VOLUME ["/data"]
WORKDIR /data

# The server shuts down gracefully on SIGINT (docker stop sends SIGTERM by default).
STOPSIGNAL SIGINT

HEALTHCHECK --interval=5s --timeout=4s --start-period=10s --retries=3 \
  CMD ["/app/necropsy-web", "healthcheck"]

ENTRYPOINT ["/app/necropsy-web"]
