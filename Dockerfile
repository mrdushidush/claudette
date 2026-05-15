# syntax=docker/dockerfile:1.7
#
# Multi-stage build for claudette. Final image is ~150 MB and runs as a
# non-root user with ~/.claudette mounted at /home/claudette/.claudette.
#
# Quick start:
#   docker build -t claudette .
#   docker run --rm -it -e OLLAMA_HOST=http://host.docker.internal:11434 \
#     -v claudette-data:/home/claudette/.claudette claudette --doctor
#
# Or use the bundled docker-compose.yml which brings up Ollama too.

ARG RUST_VERSION=1.88

# ----------------------------------------------------------------------------
# Stage 1: builder
# ----------------------------------------------------------------------------
FROM rust:${RUST_VERSION}-bookworm AS builder

WORKDIR /src

# Pre-warm the dependency cache so a code-only edit doesn't re-fetch
# every crate. The dummy main.rs lets cargo build deps before we copy
# real sources in.
COPY Cargo.toml Cargo.lock ./
COPY crates/claudette/Cargo.toml crates/claudette/Cargo.toml
RUN mkdir -p crates/claudette/src \
    && echo 'fn main() {}' > crates/claudette/src/main.rs \
    && echo '' > crates/claudette/src/lib.rs \
    && cargo build --release --locked -p claudette \
    && rm -rf crates/claudette/src target/release/deps/claudette-* \
       target/release/deps/libclaudette-*

# Real sources.
COPY crates ./crates

RUN cargo build --release --locked -p claudette \
    && cp target/release/claudette /claudette \
    && strip /claudette

# ----------------------------------------------------------------------------
# Stage 2: runtime
# ----------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

# Runtime deps:
#   ca-certificates - HTTPS to Telegram / Google / Brave (opt-in features).
#   tini            - PID 1 signal forwarding so `docker stop` reaches claudette.
#   ffmpeg + python3 + edge-tts - voice output for --telegram mode. Adds
#                     ~80 MB; remove these three lines if you don't need TTS.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        tini \
        ffmpeg \
        python3 \
        python3-pip \
    && pip3 install --no-cache-dir --break-system-packages edge-tts \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

# Non-root user. ~/.claudette is the persistence point — sessions, notes,
# recall, missions. Mount a volume there to survive container restarts.
RUN useradd --create-home --shell /bin/bash --uid 10001 claudette \
    && mkdir -p /home/claudette/.claudette \
    && chown -R claudette:claudette /home/claudette

COPY --from=builder /claudette /usr/local/bin/claudette

USER claudette
WORKDIR /home/claudette
VOLUME ["/home/claudette/.claudette"]

# Sensible defaults for a containerized deploy. Override via `docker run -e`
# or docker-compose `environment:`.
ENV OLLAMA_HOST=http://host.docker.internal:11434 \
    CLAUDETTE_SKIP_OLLAMA_PROBE=0

ENTRYPOINT ["/usr/bin/tini", "--", "claudette"]
CMD ["--doctor"]
