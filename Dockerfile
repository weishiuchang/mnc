FROM alpine:3.23 AS dev

ARG WORKDIR="/build"
ARG RUSTFLAGS="-C target-feature=+crt-static"
ARG CARGO_BUILD_TARGET=x86_64-alpine-linux-musl

# Needed for static linking
RUN set -x \
    && apk add rustfmt rust-clippy \
    && apk add --no-cache musl-dev

FROM dev AS builder

ARG WORKDIR="/build"
ARG RUSTFLAGS="-C target-feature=+crt-static"
ARG CARGO_BUILD_TARGET=x86_64-alpine-linux-musl

WORKDIR $WORKDIR
ADD . $WORKDIR/

RUN set -x \
    && cargo build --locked --target x86_64-alpine-linux-musl \
    && cargo build --release --locked --target x86_64-alpine-linux-musl

# Test
FROM dev AS test

ARG WORKDIR="/build"
ARG RUSTFLAGS="-C target-feature=+crt-static"
ARG CARGO_BUILD_TARGET=x86_64-alpine-linux-musl

WORKDIR $WORKDIR
ADD . $WORKDIR/

RUN set -x \
    && cargo test --locked --all

# Debug
FROM busybox AS debug

LABEL maintainer="davecx@gmail.com"

ENV RUST_BACKTRACE=1
ENV RUST_LOG=debug

COPY --from=builder /build/target/x86_64-alpine-linux-musl/debug/mnc /mnc

# Release
FROM scratch AS release

LABEL maintainer="davecx@gmail.com"

COPY --from=builder /build/target/x86_64-alpine-linux-musl/release/mnc /mnc

ENTRYPOINT ["/mnc"]
