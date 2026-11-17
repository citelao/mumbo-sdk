# Official Rust image on Debian (slim): rustup, cargo, gcc, and little else.
FROM docker.io/library/rust:1-slim-trixie

# git for working in the container; curl so remote-editor extensions can fetch
# their server into it (the base image strips wget after installing rustup).
RUN apt-get update \
    && apt-get install -y --no-install-recommends git curl \
    && rm -rf /var/lib/apt/lists/*

# Match CI (fmt + clippy); rust-src lets rust-analyzer index std.
RUN rustup component add clippy rustfmt rust-src

# Non-root user. The base image makes RUSTUP_HOME and CARGO_HOME
# world-writable, so rustup/cargo work without root.
ARG USERNAME=vscode
ARG UID=1000
ARG GID=1000
RUN groupadd --gid "$GID" "$USERNAME" \
    && useradd --uid "$UID" --gid "$GID" --create-home --shell /bin/bash "$USERNAME"
USER $USERNAME
