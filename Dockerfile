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
    && cargo build --release --target x86_64-alpine-linux-musl

# Debug
FROM busybox AS debug

LABEL maintainer="davecx@gmail.com"

COPY --from=builder /build/target/x86_64-alpine-linux-musl/release/mnc /mnc

# Release
FROM scratch AS release

LABEL maintainer="davecx@gmail.com"

COPY --from=builder /build/target/x86_64-alpine-linux-musl/release/mnc /mnc

ENTRYPOINT ["/mnc"]
