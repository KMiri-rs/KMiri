.PHONY: init sync status build run stop gen check miri_test

# ====== repo management ======

init:
	repo init -u git@github.com:KMiri-rs/vivoblueos-manifests.git -b kmiri -m manifest.xml && repo sync

sync:
	repo sync

status:
	repo status

MSG :=
define COMMIT_SCRIPT
if [ -n "$$(git status --porcelain)" ]; then \
    git add -A; \
    git commit -m "${MSG}"; \
fi
endef

commit:
	repo forall -p -c '${COMMIT_SCRIPT}'

# ====== docker management ======

# Run this after Dockerfile is changed.
build:
	docker compose build

run:
	@docker compose ps --status running -q vivo-blueos | grep -q . \
	  || docker compose up -d
	docker compose exec vivo-blueos bash

stop:
	docker compose stop

# ====== kernel management ======

BOARD := qemu_riscv64
BUILD_MODE := debug
OUT_DIR := out/${BOARD}.${BUILD_MODE}

gen:
	gn gen ${OUT_DIR} --args='build_type="${BUILD_MODE}" board="${BOARD}"'

check: gen
	ninja -C ${OUT_DIR} check_all

miri_test:
	@export MIRI_SYSROOT=$$(rustc --print=sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/miri-sysroot; \
		[ -d "$${MIRI_SYSROOT}" ] || { echo "$${MIRI_SYSROOT} does not exist" && exit 1; }; \
	  gn gen out/none --args='board="none"'; \
	  ninja -C out/none kernel/infra:check_infra_by_miri

# ====== rust toolchain ======
define RUST_XPY
	cd rust && $(if $(2),$(2) &&) \
	./x.py $(1) -i --stage 1 compiler/rustc && \
	./x.py $(1) -i --stage 1 library/std --target x86_64-unknown-linux-gnu && \
	./x.py $(1) -i --stage 1 library/std --target thumbv7m-none-eabi && \
	./x.py $(1) -i --stage 1 library/std --target thumbv8m.main-none-eabihf && \
	./x.py $(1) -i --stage 1 library/std --target thumbv7m-vivo-blueos-newlibeabi && \
	./x.py $(1) -i --stage 1 library/std --target thumbv8m.main-vivo-blueos-newlibeabihf && \
	./x.py $(1) -i --stage 1 library/std --target aarch64-vivo-blueos-newlib && \
	./x.py $(1) -i --stage 1 library/std --target riscv64-vivo-blueos && \
	./x.py $(1) -i --stage 1 library/std --target riscv32-vivo-blueos && \
	./x.py $(1) -i --stage 1 library/std --target riscv32imc-vivo-blueos && \
	./x.py $(1) -i --stage 1 rustfmt && \
	./x.py $(1) -i --stage 1 rust-analyzer && \
	./x.py $(1) -i --stage 1 clippy && \
	./x.py $(1) -i --stage 1 cargo && \
	./x.py $(1) -i --stage 1 miri
endef

rust-check:
	$(call RUST_XPY,check)

rust-install:
	$(call RUST_XPY,install,export DESTDIR=/tmp/sysroot && ./x.py install -i --stage 1 llvm-tools) && \
		PATH=$${DESTDIR}/usr/local/bin:$$PATH MIRI_LIB_SRC=library MIRI_SYSROOT=$${DESTDIR}/usr/local/lib/rustlib/x86_64-unknown-linux-gnu/miri-sysroot cargo miri setup

