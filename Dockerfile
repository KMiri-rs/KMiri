FROM asterinas/asterinas:0.17.0-20260114 AS asterinas

RUN apt update && \
    apt install build-essential curl gdb grub-efi-amd64 grub2-common \
        libpixman-1-dev mtools ovmf qemu-system-x86 xorriso fish vim -y

ENV VDSO_LIBRARY_DIR="/KMiri/env/linux_vdso"

WORKDIR /KMiri/asterinas

ENV RUSTFLAGS="--cfg=miri"
CMD ["make", "test"]
