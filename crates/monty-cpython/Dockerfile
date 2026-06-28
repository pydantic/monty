# Multi-stage build for the `monty-cpython` OCI image used by sandbox-control.
#
# Build context MUST be the workspace root (the monty repo root), because the
# `monty-cpython` crate has path-local dependencies on `monty`, `monty-proto`,
# and `pydantic-monty`. The `-f` flag and a workspace-root context look like:
#
#     docker buildx build --platform linux/amd64 \
#         -t ghcr.io/pydantic/monty-cpython:<sha> \
#         -f crates/monty-cpython/Dockerfile \
#         --push .
#
# Or just `make build-cpython-image` from the workspace root.
#
# Both stages share the same `python:3.14-slim-trixie` base so libpython at
# build time and runtime are exactly the same. PyO3 `auto-initialize` links
# libpython at compile time and dlopens the same major.minor at runtime — they
# must match. Debian bookworm / older bases do NOT ship Python 3.14 in apt.

ARG PYTHON_TAG=3.14-slim-trixie
ARG RUST_VERSION=1.95
# uv binary copied into the runtime image so `session.install_dependencies` /
# PEP 723 installs can shell out to it. Bump to upgrade uv.
ARG UV_VERSION=0.11.16

# ---- uv ----
# Pulled as its own stage because `COPY --from` cannot interpolate a build arg;
# the global `UV_VERSION` arg is only usable in a `FROM`. The distroless uv
# image ships the binaries at `/uv` and `/uvx`.
FROM ghcr.io/astral-sh/uv:${UV_VERSION} AS uv

# ---- builder ----
FROM python:${PYTHON_TAG} AS build

ARG RUST_VERSION

# build-essential: gcc/ld for linking the final binary
# pkg-config: pyo3-build-config uses it to locate libpython
# curl + ca-certificates: rustup + crates.io HTTPS
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        pkg-config \
        curl \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Install Rust via rustup, pinning to the workspace MSRV. `--profile minimal`
# skips docs/extra components we don't need.
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --default-toolchain ${RUST_VERSION} --profile minimal --no-modify-path

# python:3.14-slim installs Python under /usr/local, including dev headers.
# Tell pyo3-build-config exactly which interpreter to link against.
ENV PYO3_PYTHON=/usr/local/bin/python3.14

WORKDIR /src

# Bring in the entire workspace so path deps resolve. Layer caching is coarse
# — any source change invalidates the build. cargo-chef can be added later if
# rebuild time becomes a problem.
COPY . .

RUN cargo build --release -p monty-cpython --locked

# ---- runtime ----
# Same base as the builder so libpython3.14.so + stdlib match the build
# exactly.
FROM python:${PYTHON_TAG}

COPY --from=build /src/target/release/monty-cpython /usr/local/bin/monty-cpython

# uv installs the dependencies a session requests (or a fed script declares via
# PEP 723). The worker invokes the `uv` binary on PATH; `/usr/local/bin` is on
# the default PATH, so no `MONTY_UV` override is needed.
COPY --from=uv /uv /uvx /usr/local/bin/

# Make uv resolve installs against this image's system Python 3.14 (the exact
# interpreter the worker embeds) rather than downloading a managed one — same
# ABI, no surprise interpreter fetch on the first install.
ENV UV_PYTHON_PREFERENCE=only-system

# The worker installs a session's dependencies into a virtualenv at `./.venv`,
# relative to its working directory. Pre-create it here (and pin it to this
# image's Python 3.14, matching the embedded libpython) so the first install
# does not pay venv-creation cost. WORKDIR fixes the runtime CWD so the worker
# resolves `./.venv` to this directory. A per-session sandbox gives each worker
# its own writable copy, so installs into the venv stay session-local.
WORKDIR /app
RUN uv venv .venv --python 3.14

# Smoke test at build time — fails the build if the binary cannot dlopen
# libpython or otherwise refuses to start, if uv is not runnable, or if the
# virtualenv was not created.
RUN monty-cpython --help >/dev/null && uv --version >/dev/null && test -d /app/.venv

# sandbox-control execs the binary directly with argv
# ["monty-cpython", "websocket", "<url>"], so an ENTRYPOINT is optional but
# convenient for local debugging.
ENTRYPOINT ["monty-cpython"]
