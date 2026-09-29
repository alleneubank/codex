FROM ubuntu:24.04@sha256:52df9b1ee71626e0088f7d400d5c6b5f7bb916f8f0c82b474289a4ece6cf3faf

ARG DEBIAN_FRONTEND=noninteractive
ARG RUST_VERSION=1.95.0
ARG ZIG_VERSION=0.14.0
ARG ZIG_SHA256=473ec26806133cf4d1918caf1a410f8403a13d979726a9045b421b685031a982

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        binutils \
        build-essential \
        ca-certificates \
        clang \
        curl \
        file \
        g++ \
        git \
        libcap-dev \
        libc++-dev \
        libc++abi-dev \
        lld \
        musl-tools \
        pkg-config \
        python3 \
        sudo \
        xz-utils \
        zstd \
    && rm -rf /var/lib/apt/lists/*

RUN curl -fsSL "https://ziglang.org/download/${ZIG_VERSION}/zig-linux-x86_64-${ZIG_VERSION}.tar.xz" \
        -o /tmp/zig.tar.xz \
    && echo "${ZIG_SHA256}  /tmp/zig.tar.xz" | sha256sum -c - \
    && mkdir -p /opt/zig \
    && tar -xJf /tmp/zig.tar.xz --strip-components=1 -C /opt/zig \
    && ln -s /opt/zig/zig /usr/local/bin/zig \
    && rm /tmp/zig.tar.xz

ENV CARGO_HOME=/cargo
ENV RUSTUP_HOME=/rustup
ENV PATH=/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
RUN curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs \
        | sh -s -- -y --profile minimal --default-toolchain "${RUST_VERSION}" \
    && rustup target add --toolchain "${RUST_VERSION}" x86_64-unknown-linux-musl \
    && rustc --version \
    && zig version

WORKDIR /workspace/codex-rs
