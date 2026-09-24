PROJ := $(abspath $(dir $(lastword $(MAKEFILE_LIST)))/..)

# ====== repo management ======
.PHONY: init sync status snapshot restore commit

init:
	repo init -u git@github.com:KMiri-rs/vivoblueos-manifests.git -b kmiri -m manifest.xml && repo sync

sync:
	repo sync

status:
	repo status

# For reproduce.
snapshot:
	repo manifest -o snapshot.xml -r
restore:
	repo init -m snapshot.xml && repo sync

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
.PHONY: build run stop gen check miri_test

# Run this after Dockerfile is changed.
build:
	docker compose build

DOCKER_SERVICE := vivo-blueos_v3
run:
	@docker compose ps --status running -q $(DOCKER_SERVICE) | grep -q . \
	  || docker compose up -d
	docker compose exec $(DOCKER_SERVICE) bash

stop:
	docker compose stop

# ====== kernel management ======
.PHONY: gen check kmiri-setup miri_test fmt rustfmt gn-check-format

BOARD := qemu_riscv64
BUILD_MODE := debug
OUT_DIR := out/${BOARD}.${BUILD_MODE}

gen:
	gn gen ${OUT_DIR} --args='build_type="${BUILD_MODE}" board="${BOARD}"'

check: gen
	ninja -C ${OUT_DIR} check_all

RUST_LIB_SRC := $(abspath rust/library)

rust-analyzer:
	gn gen out/$(BOARD) --export-rust-project
	python3 -c "\
import json, sys; \
d = json.load(open('out/$(BOARD)/rust-project.json')); \
d['sysroot_src'] = '$(RUST_LIB_SRC)'; \
json.dump(d, open('out/$(BOARD)/rust-project.json', 'w'), indent=2)"
	cp out/$(BOARD)/rust-project.json rust-project.json

kmiri-setup:
	make -C /KMiri/kmiri kmiri-setup

CASE :=
MIRI_FLAGS := ["-Zkmiri-toml=/KMiri/vivo-blueos/kmiri.toml"]
miri_test: kmiri-setup
	export LD_LIBRARY_PATH=$$(rustc --print=sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/lib; \
	export MIRI_SYSROOT=/root/.cache/miri; \
	[ -d "$${MIRI_SYSROOT}" ] || { echo "$${MIRI_SYSROOT} does not exist" && exit 1; }; \
	rm out/ -rf; \
	gn gen out/none --args='board="none" miri_test_case="$(CASE)" miri_flags=$(MIRI_FLAGS)'; \
	ninja -C out/none kernel/infra:check_infra_by_miri

# ====== code formatting ======
fmt: rustfmt gn-check-format

RUST_KERNEL_SRC := $(abspath kernel)

rustfmt:
	@echo "Running rustfmt on kernel Rust files..."
	find $(RUST_KERNEL_SRC) -type f -name "*.rs" | \
		xargs rustfmt --edition=2021 --unstable-features --skip-children \
			--config-path=$(RUST_KERNEL_SRC)/rustfmt.toml || true

gn-check-format:
	@echo "Checking GN file formatting..."
	find build -type f \( -name "*.gn" -o -name "*.gni" \) -exec gn format --dry-run {} +

# ====== rust toolchain ======
.PHONY: rust-check rust-install rust-sysroot-tarball rust-sysroot-upload rust-sysroot-download

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
	$(call RUST_XPY,install,export DESTDIR=/tmp/sysroot5 && \
		./x.py install -i --stage 1 llvm-tools && \
		./x.py dist rustc-dev && \
		cp -r build/tmp/tarball/rust-src/image/lib/rustlib $${DESTDIR}/usr/local/lib/ && \
		cp -r build/tmp/tarball/rustc-dev/x86_64-unknown-linux-gnu/image/lib/rustlib $${DESTDIR}/usr/local/lib/) && \
		PATH=$${DESTDIR}/usr/local/bin:$${PATH} \
		MIRI_SYSROOT=$${DESTDIR}/usr/local/lib/rustlib/x86_64-unknown-linux-gnu/miri-sysroot \
		cargo miri setup

SYSROOT_RUST_TOOLCHAIN := rustc-sysroot-nightly-2026-07-21
rust-sysroot-tarball:
	tar -cf - sysroot/ | xz -9 > $(SYSROOT_RUST_TOOLCHAIN).tar.xz

rust-sysroot-upload:
	gh release create $(SYSROOT_RUST_TOOLCHAIN)_v3 \
		$(SYSROOT_RUST_TOOLCHAIN).tar.xz \
		--title "$(SYSROOT_RUST_TOOLCHAIN)" \
		--notes "$(SYSROOT_RUST_TOOLCHAIN)" \
		-R KMiri-rs/vivoblueos-toolchain

rust-sysroot-download:
	wget https://github.com/KMiri-rs/vivoblueos-toolchain/releases/download/rustc-sysroot-nightly-2026-07-21/rustc-sysroot-nightly-2026-07-21.tar.xz
	rm -rf sysroot
	tar -xJf rustc-sysroot-nightly-2026-07-21.tar.xz
