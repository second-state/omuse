# OMuse (Rust) as a single container: the `omuse` binary runs Sentinel, the agent runtime and the browser broker in
# one process, the same three services as the Olares pod. Build and run:
#   docker build -t omuse .
#   docker run -d --name omuse --restart unless-stopped -p 8080:8080 -v omuse-data:/omuse \
#     -v $PWD/config.toml:/omuse/config.toml:ro omuse
# Everything the services need from the host is in config.toml (see config.example.toml). Without the file, the
# entrypoint writes one from OMUSE_PASSWORD, OMUSE_MODEL_URL, ... (for platforms that only pass environment variables).
#
# External programs the binary calls (all listed here, nothing is downloaded at run time):
#   chromium + xvfb  : the agent's browser (headed on a virtual display; headless when [browser] headless = "1")
#   ffmpeg           : video frames / audio for attachments
#   heif-convert     : HEIC/HEIF photos → JPEG (libheif-examples)
#   fonts-noto-cjk   : Chinese/Japanese/Korean text in screenshots and PDFs
#   tini, curl       : init process; the HEALTHCHECK
FROM rust:1-bookworm AS builder
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY web ./web
COPY skills ./skills
# Release builds pass the git tag (e.g. locius-v0.3.1); the app shows it as v0.3.1. Empty = version from the source.
ARG OMUSE_VERSION=
ENV OMUSE_VERSION=$OMUSE_VERSION
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo build --release --locked --bin omuse && cp /src/target/release/omuse /omuse-bin

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates tini tzdata curl \
      chromium xvfb fonts-noto-cjk fonts-noto-color-emoji fonts-liberation \
      ffmpeg libheif-examples \
    && rm -rf /var/lib/apt/lists/*
# the unprivileged service user
RUN useradd --system --create-home --home-dir /home/omuse --uid 1000 --shell /usr/sbin/nologin omuse
COPY --from=builder /omuse-bin /opt/omuse/bin/omuse
COPY deploy/docker/entrypoint.sh /opt/omuse/entrypoint.sh
COPY config.example.toml /opt/omuse/config.example.toml
RUN chmod 0755 /opt/omuse/bin/omuse /opt/omuse/entrypoint.sh
# all state (vault, audit log, memory, workspace files, browser profile) lives in one volume, next to config.toml
RUN mkdir -p /omuse && chown omuse:omuse /omuse
VOLUME /omuse
USER omuse
ENV HOME=/home/omuse OMUSE_CONFIG=/omuse/config.toml
# 8080: web UI (password protected). 8083: phone-call media/webhooks, only needed for the Telnyx phone line.
EXPOSE 8080 8083
HEALTHCHECK --interval=30s --timeout=5s --start-period=40s --retries=3 \
    CMD curl -fsS http://127.0.0.1:8080/sentinel/api/health || exit 1
ENTRYPOINT ["tini", "--", "/opt/omuse/entrypoint.sh"]
