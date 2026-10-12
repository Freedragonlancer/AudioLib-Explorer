# syntax=docker/dockerfile:1
#
# Multi-stage image for audiolib-server.
#
# Build from the REPOSITORY ROOT (the server depends on crates/ via path dependencies):
#   docker build -f server/deploy/docker/Dockerfile -t audiolib-server .
#   docker build -f server/deploy/docker/Dockerfile --build-arg WITH_FFMPEG=false -t audiolib-server:slim .
#
# Build requirements (installed below): build-essential, pkg-config, clang + libclang-dev,
# libasound2-dev. The ALSA headers are only needed because audiolib-audio currently always
# pulls in cpal; the server never opens an audio device. Once audiolib-audio has a
# `playback` cargo feature that the server builds without, libasound2-dev (and pkg-config)
# can be dropped from the builder stage.
ARG RUST_VERSION=1.97.1
ARG DEBIAN_RELEASE=bookworm

FROM rust:${RUST_VERSION}-${DEBIAN_RELEASE} AS builder
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      build-essential pkg-config clang libclang-dev libasound2-dev git \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
# Keep .git in the build context so /health reports the real commit (else "unknown").
COPY . .
# The repository's .cargo/config.toml pins a Windows default target, so pass the host
# target explicitly (also makes the output path predictable).
RUN HOST_TARGET="$(rustc -vV | sed -n 's/^host: //p')" \
 && cargo build --release --locked -p audiolib-server --target "$HOST_TARGET" \
 && install -D -m 755 "target/$HOST_TARGET/release/audiolib-server" /out/audiolib-server

FROM debian:${DEBIAN_RELEASE}-slim AS runtime
# ffmpeg is OPTIONAL: it is only used for /audio/{id}/preview?codec=flac. Without it the
# server still runs and answers 503 TRANSCODER_UNAVAILABLE for that codec. Note that the
# distribution's ffmpeg is usually a GPL build; set WITH_FFMPEG=false for a slim image.
ARG WITH_FFMPEG=true
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl libasound2 \
 && if [ "$WITH_FFMPEG" = "true" ]; then apt-get install -y --no-install-recommends ffmpeg; fi \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --home-dir /data --shell /usr/sbin/nologin audiolib \
 && install -d -o audiolib -g audiolib /data /config /music
COPY --from=builder /out/audiolib-server /usr/local/bin/audiolib-server
COPY server/deploy/docker/config.docker.toml /config/config.toml
USER audiolib
WORKDIR /data
VOLUME ["/data"]
EXPOSE 8790
ENV AUDIOLIB_SERVER_CONFIG=/config/config.toml
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --retries=3 \
  CMD curl -fsS http://127.0.0.1:8790/api/v1/health || exit 1
ENTRYPOINT ["audiolib-server"]
