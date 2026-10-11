# syntax=docker/dockerfile:1
#
# One service per image (T-1104): this file builds `api`, `unsub` and `worker`
# from the `SERVICE` build argument, so there is a single definition of how a
# MailTinder service is packaged.
#
#   docker build --build-arg SERVICE=api -f backend/Dockerfile backend
#
# The build context is `backend/`, and `.dockerignore` keeps `target/`, the
# tests and the fixtures out of it.
#
# ASVS V13.4.1: the final stage holds the release binary and nothing else - no
# source tree, no toolchain, no package manager, no shell and no `.git`, and it
# runs as the distroless `nonroot` user. The policy test
# `crates/api/tests/dockerfile_policy.rs` fails if any of that changes.
#
# ASVS V13.4.2: the binary is built `--release --locked` with no feature
# selection, so the e2e entry points and every other debug affordance are not
# compiled into the shipped image.

# ---- build -----------------------------------------------------------------
FROM rust:1.99.0-bookworm AS build
ARG SERVICE
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p "${SERVICE}"

# ---- runtime ----------------------------------------------------------------
FROM gcr.io/distroless/cc-debian12:nonroot
ARG SERVICE
COPY --from=build /src/target/release/${SERVICE} /app
USER nonroot
ENTRYPOINT ["/app"]
