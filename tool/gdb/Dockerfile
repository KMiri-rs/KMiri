FROM ubuntu:latest AS builder
ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y \
    build-essential git bison flex texinfo libgmp-dev libmpfr-dev libmpc-dev libexpat1-dev \
    libncurses5-dev python3-dev zlib1g-dev guile-3.0-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /gdb
RUN git clone --depth 1 https://sourceware.org/git/binutils-gdb.git .

WORKDIR /build
RUN /gdb/configure \
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
    --disable-werror
RUN make -j$(nproc)
RUN make install
