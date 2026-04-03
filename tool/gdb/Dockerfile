FROM ubuntu:latest AS builder
ENV DEBIAN_FRONTEND=noninteractive

# Prepare gdb enviroment.
RUN apt-get update \
    && apt-get install -y \
        build-essential git bison flex texinfo libgmp-dev libmpfr-dev \
        libmpc-dev libexpat1-dev libncurses5-dev python3-dev zlib1g-dev guile-3.0-dev \
    && rm -rf /var/lib/apt/lists/*

# Install latest gdb from source code.
WORKDIR /gdb
RUN git clone --depth 1 https://sourceware.org/git/binutils-gdb.git . \
    && mkdir build && cd build \
    && ../configure \
        --prefix=/usr/local \
        --enable-gold \
        --enable-plugins \
        --enable-targets=all \
        --enable-64-bit-bfd \
        --enable-sim \
        --with-python=python3 \
        --with-guile \
        --enable-tui \
        --with-source-highlight \
        --with-expat \
        --disable-werror \
    && make -j$(nproc) \
    && make install \
    && cd / && rm /gdb -rf

WORKDIR /
# Install Rust and dev tools.
RUN apt update && apt install -y fish less vim curl \
    && curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- --default-toolchain none -y \
    && . "$HOME/.cargo/env" \
    && rustup default nightly-2025-12-06 \
    && rustup component add miri \

