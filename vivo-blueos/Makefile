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
	docker compose exec $(DOCKER_SERVICE) fish

stop:
	docker compose stop

# ====== kernel management ======
.PHONY: gen check kmiri-setup miri_test miri_test_kernel miri_test_allocator fmt rustfmt gn-check-format

BOARD := qemu_riscv64
BUILD_TYPE := debug
OUT_DIR := out/${BOARD}.${BUILD_TYPE}
DEFCONFIG := $(abspath minimal.defconfig)
ARGS_BASE := board=\"$(BOARD)\" build_type=\"$(BUILD_TYPE)\" defconfig_files=\"$(DEFCONFIG)\"

gen:
	gn gen ${OUT_DIR} --args='build_type="${BUILD_TYPE}" board="${BOARD}"'

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

# Optional miri flags:
# DEBUGGER=1 adds --debugger (interactive TUI),
# TB=1 adds -Zmiri-tree-borrows (Tree Borrows aliasing model).
DEBUGGER :=
TB :=

# Emits ', "$2"' only when $1 is non-empty, so any combination is a valid GN list.
_sep_ := ,
add_flag = $(if $(1),$(_sep_) "$(2)")
MIRI_FLAGS := [ \
	"-Zkmiri-toml=/KMiri/vivo-blueos/kmiri.toml" \
	$(call add_flag,$(DEBUGGER),--debugger) \
	$(call add_flag,$(TB),-Zmiri-tree-borrows) \
]
miri_test: kmiri-setup
	export LD_LIBRARY_PATH=$$(rustc --print=sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/lib; \
	export MIRI_SYSROOT=/root/.cache/miri; \
	[ -d "$${MIRI_SYSROOT}" ] || { echo "$${MIRI_SYSROOT} does not exist" && exit 1; }; \
	rm out/ -rf; \
	gn gen out/none --args='board="none" miri_test_case="$(CASE)" miri_flags=$(MIRI_FLAGS)'; \
	ninja -C out/none kernel/infra:check_infra_by_miri

miri_test_allocator: kmiri-setup
	export LD_LIBRARY_PATH=$$(rustc --print=sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/lib; \
	export MIRI_SYSROOT=/root/.cache/miri; \
	[ -d "$${MIRI_SYSROOT}" ] || { echo "$${MIRI_SYSROOT} does not exist" && exit 1; }; \
	out=out/miri_test_allocator; \
	rm $${out} -rf; \
	gn gen $${out} --args='board="none" defconfig_files="$(DEFCONFIG)" miri_test_allocator=true allocator_miri_test_case="$(CASE)" miri_flags=$(MIRI_FLAGS)'; \
	ninja -C $${out} kernel/allocator:check_allocator_by_miri

miri_run: kmiri-setup
	rm out/ -rf; \
	gn gen out/miri-boot/native-minimal --args="$(ARGS_BASE)"; 
	ninja -C out/miri-boot/native-minimal apps/example/no_std:no_std_example

# Boot and run the BlueOS kernel under Miri (KMiri), not QEMU.
#
# It is a two-stage build:
#   stage 1 (native)  - GN compiles the kernel crate graph with plain rustc,
#                       every rlib carrying MIR via `-Zalways-encode-mir`
#                       (see build/config/BUILD.gn under `miri_boot`). The
#                       `no_std_example` ELF is only a native reference.
#   stage 2 (interpret)- check_boot_by_miri runs build/scripts/miri/boot.py,
#                       which reuses GN's own rustc flags (via
#                       `ninja -t commands`), forces `--crate-type=bin`, and
#                       lets miri interpret the kernel from its `miri_start`
#                       entry under the boot.toml memory model.
#
# Key environment variables:
#   MIRI_BOOT_SYSROOT - no_std RISC-V sysroot with full MIR; read by gn
#                       (build/config/BUILD.gn) and by boot.py. Created here
#                       with `cargo miri setup` when missing.
#   MIRI_SYSROOT      - consumed by `cargo miri setup` to choose the sysroot
#                       location, and by the miri driver; unset while the
#                       native stage builds so plain rustc is used.
#   LD_LIBRARY_PATH   - miri needs it to load librustc_driver at runtime.
#   PATH              - boot.py finds the `miri` executable on PATH.
MIRI_BOOT_OUT := $(abspath out/miri-boot)
MIRI_BOOT_SYSROOT := $(MIRI_BOOT_OUT)/sysroot
MIRI_BOOT_NATIVE  := $(MIRI_BOOT_OUT)/native-minimal
MIRI_BOOT_NATIVE_STAMP := $(MIRI_BOOT_NATIVE)/obj/kernel/kernel/blueos/libblueos.rlib
MIRI_KERNEL_TEST_OUT := $(abspath out/miri-kernel-test)

.PHONY: miri_boot
miri_boot: kmiri-setup
	export LD_LIBRARY_PATH=$$(rustc --print=sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/lib; \
	if [ ! -d "$${MIRI_BOOT_SYSROOT}/lib/rustlib/riscv64-vivo-blueos/lib" ]; then \
		MIRI_SYSROOT=$(MIRI_BOOT_SYSROOT) cargo miri setup --target riscv64-vivo-blueos; \
	fi; \
	if [ ! -f "$(MIRI_BOOT_NATIVE_STAMP)" ]; then \
		gn gen $(MIRI_BOOT_NATIVE) --args="$(ARGS_BASE)"; \
		ninja -C $(MIRI_BOOT_NATIVE) kernel/kernel:blueos; \
	fi; \
	export MIRI_BOOT_SYSROOT=$(MIRI_BOOT_SYSROOT); \
	gn gen "$(MIRI_BOOT_OUT)" --args="$(ARGS_BASE) miri_boot=true"; \
	ninja -C "$(MIRI_BOOT_OUT)" kernel/kernel:check_boot_by_miri

.PHONY: miri_test_kernel
miri_test_kernel: kmiri-setup
	export LD_LIBRARY_PATH=$$(rustc --print=sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/lib; \
	export MIRI_SYSROOT=/root/.cache/miri; \
	[ -d "$${MIRI_SYSROOT}" ] || { echo "$${MIRI_SYSROOT} does not exist" && exit 1; }; \
	export TARGET_BOARD=miri; \
	out=$(MIRI_KERNEL_TEST_OUT); \
	rm -rf $${out}; \
	gn gen $${out} --args='board="miri" build_type="$(BUILD_TYPE)" defconfig_files="$(DEFCONFIG)" miri_test_kernel=true kernel_miri_test_case="$(CASE)" miri_flags=$(MIRI_FLAGS)'; \
	ninja -C $${out} kernel/kernel:check_kernel_by_miri

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
	find build -type f \( -name "*.gn" -o -name "*.gni" \) -exec gn format {} +

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
