PROJ := $(abspath $(dir $(lastword $(MAKEFILE_LIST)))/..)
SYSROOT := $(shell rustc --print sysroot)
MIRI_SYSROOT_REBUILT := /root/.cache/miri

ASTERINAS_TOML := $(PROJ)/tests/asterinas.toml
ASTERINAS_ENV_VARS := OSDK_LOCAL_DEV=1 MIRIFLAGS="-Zkmiri-toml=$(ASTERINAS_TOML) -Zmiri-permissive-provenance -Zmiri-tree-borrows"

define RUN_IRQ_RCU_UAF_TEST
log=$$(mktemp); \
if (cd $(PROJ)/tests/irq-rcu-uaf && $(ASTERINAS_ENV_VARS) cargo osdk miri test) >"$$log" 2>&1; then \
	cat "$$log"; rm -f "$$log"; echo "irq-rcu-uaf unexpectedly completed without aborting"; exit 1; \
fi; \
cat "$$log"; \
if awk '/Undefined Behavior: / && /dangling reference \(use-after-free\)/ { found = 1 } END { exit !found }' "$$log"; then \
	echo "Observed expected RCU use-after-free diagnostic."; rm -f "$$log"; \
else \
	rm -f "$$log"; echo "irq-rcu-uaf failed without the expected use-after-free diagnostic"; exit 1; \
fi
endef

# `vmspace-activate-irq` needs injected timer interrupts, so it and its control
# run with that test's own kmiri.toml.
VMSPACE_IRQ_ENV_VARS := OSDK_LOCAL_DEV=1 MIRIFLAGS="-Zkmiri-toml=$(PROJ)/tests/vmspace-activate-irq/kmiri.toml -Zmiri-permissive-provenance -Zmiri-tree-borrows"

define RUN_VMSPACE_ACTIVATE_IRQ_TEST
log=$$(mktemp); \
if (cd $(PROJ)/tests/vmspace-activate-irq && $(VMSPACE_IRQ_ENV_VARS) cargo osdk miri test) >"$$log" 2>&1; then \
	cat "$$log"; rm -f "$$log"; echo "vmspace-activate-irq unexpectedly completed without aborting"; exit 1; \
fi; \
cat "$$log"; \
if { awk '/Undefined Behavior: / && (/has been freed/ || /strongly protected/) { found = 1 } END { exit !found }' "$$log" || \
     { grep -qF "protected tags must never be Disabled" "$$log" && grep -qF "core::ptr::drop_in_place::<ostd::mm::VmSpace>" "$$log"; }; } && \
   grep -F "ostd::mm::VmSpace::activate" "$$log" >/dev/null; then \
	echo "Observed expected Undefined Behavior in VmSpace::activate."; rm -f "$$log"; \
else \
	rm -f "$$log"; echo "vmspace-activate-irq failed without the expected Undefined Behavior in VmSpace::activate"; exit 1; \
fi
endef

define RUN_VMSPACE_ACTIVATE_IRQ_CONTROL_TEST
cd $(PROJ)/tests/vmspace-activate-irq-control && $(VMSPACE_IRQ_ENV_VARS) cargo osdk miri test
endef

# The SMP tests run with three CPUs, set in `tests/smp-ap-tasks/kmiri.toml`.
SMP_ENV_VARS := OSDK_LOCAL_DEV=1 MIRIFLAGS="-Zkmiri-toml=$(PROJ)/tests/smp-ap-tasks/kmiri.toml -Zmiri-permissive-provenance -Zmiri-tree-borrows"

define RUN_SMP_AP_TASKS_TEST
cd $(PROJ)/tests/smp-ap-tasks && $(SMP_ENV_VARS) cargo osdk miri test
endef

# An AP whose last task returns stops; its boot context is not a leak.
define RUN_SMP_AP_TASK_RETURNS_TEST
cd $(PROJ)/tests/smp-ap-task-returns && $(SMP_ENV_VARS) cargo osdk miri test
endef

SMP_IPI_CALL_ENV_VARS := OSDK_LOCAL_DEV=1 MIRIFLAGS="-Zkmiri-toml=$(PROJ)/tests/smp-ipi-call/kmiri.toml -Zmiri-permissive-provenance -Zmiri-tree-borrows"

define RUN_SMP_IPI_CALL_TEST
cd $(PROJ)/tests/smp-ipi-call && $(SMP_IPI_CALL_ENV_VARS) cargo osdk miri test
endef

SMP_TLB_SHOOTDOWN_ENV_VARS := OSDK_LOCAL_DEV=1 MIRIFLAGS="-Zkmiri-toml=$(PROJ)/tests/smp-tlb-shootdown/kmiri.toml -Zmiri-permissive-provenance -Zmiri-tree-borrows"

define RUN_SMP_TLB_SHOOTDOWN_TEST
cd $(PROJ)/tests/smp-tlb-shootdown && $(SMP_TLB_SHOOTDOWN_ENV_VARS) cargo osdk miri test
endef

# Both sides of the reported race must be the unsynchronized `+= 1` on the
# counter, which the diagnostic quotes once for each side.
define RUN_SMP_DATA_RACE_TEST
log=$$(mktemp); \
if (cd $(PROJ)/tests/smp-data-race && $(SMP_ENV_VARS) cargo osdk miri test) >"$$log" 2>&1; then \
	cat "$$log"; rm -f "$$log"; echo "smp-data-race unexpectedly completed without aborting"; exit 1; \
fi; \
cat "$$log"; \
if grep -q "Undefined Behavior: Data race detected" "$$log" && [ "$$(grep -cF 'unsafe { *COUNTER.0.get() += 1 }' "$$log")" -ge 2 ]; then \
	echo "Observed expected data race on the counter."; rm -f "$$log"; \
else \
	rm -f "$$log"; echo "smp-data-race failed without the expected data race on the counter"; exit 1; \
fi
endef

# A 96 KiB CPU-local variable runs with the larger `cpu_local_size` in the test's
# `kmiri.toml`, and is reported with the size to set under the default 64 KiB.
define RUN_CPU_LOCAL_LARGE_SECTION_TEST
(cd $(PROJ)/tests/cpu-local-large-section && OSDK_LOCAL_DEV=1 MIRIFLAGS="-Zkmiri-toml=$(PROJ)/tests/cpu-local-large-section/kmiri.toml -Zmiri-permissive-provenance -Zmiri-tree-borrows" cargo osdk miri test); \
log=$$(mktemp); \
if (cd $(PROJ)/tests/cpu-local-large-section && $(ASTERINAS_ENV_VARS) cargo osdk miri test) >"$$log" 2>&1; then \
	cat "$$log"; rm -f "$$log"; echo "cpu-local-large-section unexpectedly fit in the default section"; exit 1; \
fi; \
cat "$$log"; \
if grep -q "the CPU-local section is full" "$$log"; then \
	echo "Observed expected CPU-local section diagnostic."; rm -f "$$log"; \
else \
	rm -f "$$log"; echo "cpu-local-large-section failed without the expected diagnostic"; exit 1; \
fi
endef

# A fallible atomic load of a never-written word on an untyped frame reads
# uninitialized memory into a `u32`.
define RUN_FRAME_ATOMIC_UNINIT_TEST
log=$$(mktemp); \
if (cd $(PROJ)/tests/frame-atomic-uninit && $(ASTERINAS_ENV_VARS) cargo osdk miri test) >"$$log" 2>&1; then \
	cat "$$log"; rm -f "$$log"; echo "frame-atomic-uninit unexpectedly completed without aborting"; exit 1; \
fi; \
cat "$$log"; \
if grep -q "Undefined Behavior: .*memory is uninitialized" "$$log" && grep -qF "ostd::arch::mm::__atomic_load_fallible" "$$log"; then \
	echo "Observed expected uninitialized read in the fallible atomic load."; rm -f "$$log"; \
else \
	rm -f "$$log"; echo "frame-atomic-uninit failed without the expected uninitialized read"; exit 1; \
fi
endef

# The machine in `tests/ram-size-small/kmiri.toml` has 64 MiB of RAM.
define RUN_RAM_SIZE_SMALL_TEST
(cd $(PROJ)/tests/ram-size-small && OSDK_LOCAL_DEV=1 MIRIFLAGS="-Zkmiri-toml=$(PROJ)/tests/ram-size-small/kmiri.toml -Zmiri-permissive-provenance -Zmiri-tree-borrows" cargo osdk miri test)
endef

# The kernel drives the virtio block device of its own `kmiri.toml` with safe
# code only, and makes it write an immutable static by DMA (Asterinas #3905).
# A control read into DMA memory must succeed first; then KMiri must report the
# write to read-only memory at the MMIO write that notifies the device.
define RUN_DMA_IMMUTABLE_STATIC_TEST
log=$$(mktemp); \
if (cd $(PROJ)/tests/dma-immutable-static && OSDK_LOCAL_DEV=1 MIRIFLAGS="-Zkmiri-toml=$(PROJ)/tests/dma-immutable-static/kmiri.toml -Zmiri-permissive-provenance -Zmiri-tree-borrows" cargo osdk miri test) >"$$log" 2>&1; then \
	cat "$$log"; rm -f "$$log"; echo "dma-immutable-static unexpectedly completed without aborting"; exit 1; \
fi; \
cat "$$log"; \
if grep -qF "dma-immutable-static: control ok" "$$log" && grep -q "Undefined Behavior: writing to .* which is read-only" "$$log" && grep -qF "dma_immutable_static::Blk::read_sector" "$$log"; then \
	echo "Observed expected DMA write to an immutable static."; rm -f "$$log"; \
else \
	rm -f "$$log"; echo "dma-immutable-static failed without the expected DMA write to read-only memory"; exit 1; \
fi
endef

# Safe `IoMem<Insensitive>::read_once` with an unsupported 3-byte `PodOnce` type
# reaches `core::hint::unreachable_unchecked` (Asterinas #3894).
# A control 4-byte read/write must succeed first; then KMiri must report
# entering unreachable code through `iomem_podonce_size::kernel_main`.
define RUN_IOMEM_PODONCE_SIZE_TEST
log=$$(mktemp); \
if (cd $(PROJ)/tests/iomem-podonce-size && $(ASTERINAS_ENV_VARS) cargo osdk miri test) >"$$log" 2>&1; then \
	cat "$$log"; rm -f "$$log"; echo "iomem-podonce-size unexpectedly completed without aborting"; exit 1; \
fi; \
cat "$$log"; \
if grep -qF "iomem-podonce-size: control ok" "$$log" && grep -qF "Undefined Behavior: entering unreachable code" "$$log" && grep -qF "iomem_podonce_size::kernel_main" "$$log"; then \
	echo "Observed expected unreachable_unchecked UB for unsupported PodOnce size."; rm -f "$$log"; \
else \
	rm -f "$$log"; echo "iomem-podonce-size failed without the expected unreachable_unchecked UB"; exit 1; \
fi
endef

# Runs the ktests of tests/ktest-runner through tool/ktest-sweep.py and checks
# that each one gets the expected result, including the test after the one that
# hits Undefined Behavior.

# `ostd::task::scheduler::kernel_task_entry` is an internal cfg(miri) thread
# entry point and must not be callable from outside `ostd`.
define RUN_KERNEL_TASK_ENTRY_PRIVATE_TEST
log=$$(mktemp); \
if (cd $(PROJ)/tests/kernel-task-entry-private && $(ASTERINAS_ENV_VARS) cargo osdk miri test) >"$$log" 2>&1; then \
	cat "$$log"; rm -f "$$log"; echo "kernel-task-entry-private unexpectedly compiled and passed"; exit 1; \
fi; \
cat "$$log"; \
if grep -qF "error[E0603]: function \`kernel_task_entry\` is private" "$$log"; then \
	echo "Observed expected privacy error for kernel_task_entry."; rm -f "$$log"; \
else \
	rm -f "$$log"; echo "kernel-task-entry-private failed without the expected E0603 privacy error"; exit 1; \
fi
endef

define RUN_KTEST_RUNNER_TEST
out=$$(mktemp -d); \
(cd $(PROJ)/tests/ktest-runner && $(ASTERINAS_ENV_VARS) python3 $(PROJ)/tool/ktest-sweep.py . "$$out") || true; \
got=$$(awk -F'\t' '{ sub(/.*::/, "", $$1); print $$1 " " $$2 }' "$$out/ledger.tsv" | sort | tr '\n' ' '); \
want="a_passes ok b_fails FAILED c_use_after_free ABORT d_passes_after_ub ok "; \
if [ "$$got" = "$$want" ] && grep -q "has been freed" "$$out/ledger.tsv"; then \
	echo "ktest-runner: PASS"; \
else \
	echo "ktest-runner: FAIL: got '$$got', want '$$want'"; cat "$$out/ledger.tsv"; exit 1; \
fi
endef

DEBUG := --debug

.PHONY: install
install:
	[ -d "$(MIRI_SYSROOT_REBUILT)" ] && MIRI_SYSROOT="$(MIRI_SYSROOT_REBUILT)" || MIRI_SYSROOT="$(SYSROOT)"; \
	export MIRI_SYSROOT; \
	cd $(PROJ)/kmiri && ./miri install $(DEBUG) && \
	cd $(PROJ)/kmiri-helper && cargo install --path .

.PHONY: install-osdk
install-osdk:
	cd $(PROJ)/asterinas && OSDK_LOCAL_DEV=1 make install_osdk

.PHONY: asterinas
ASTERINAS_TEST := init
ifeq ($(ASTERINAS_TEST),irq-rcu-uaf)
asterinas: install install-osdk
	$(RUN_IRQ_RCU_UAF_TEST)

else ifeq ($(ASTERINAS_TEST),vmspace-activate-irq)
asterinas: install install-osdk
	$(RUN_VMSPACE_ACTIVATE_IRQ_TEST)

else ifeq ($(ASTERINAS_TEST),vmspace-activate-irq-control)
asterinas: install install-osdk
	$(RUN_VMSPACE_ACTIVATE_IRQ_CONTROL_TEST)

else ifeq ($(ASTERINAS_TEST),smp-ap-tasks)
asterinas: install install-osdk
	$(RUN_SMP_AP_TASKS_TEST)

else ifeq ($(ASTERINAS_TEST),smp-ipi-call)
asterinas: install install-osdk
	$(RUN_SMP_IPI_CALL_TEST)

else ifeq ($(ASTERINAS_TEST),smp-tlb-shootdown)
asterinas: install install-osdk
	$(RUN_SMP_TLB_SHOOTDOWN_TEST)

else ifeq ($(ASTERINAS_TEST),smp-data-race)
asterinas: install install-osdk
	$(RUN_SMP_DATA_RACE_TEST)

else
asterinas: install install-osdk
	echo $(ASTERINAS_TEST)
	cd $(PROJ)/tests/$(ASTERINAS_TEST) && \
		$(ASTERINAS_ENV_VARS) cargo osdk miri test
endif

ASTERINAS_TESTS := init frame-realloc page-table-realloc segment-zeroed \
                   task-switch-back task-exit-last task-exit-with-runnable \
                   irq-simulation irq-trap-path irq-rcu-uaf \
                   irq-rcu-uaf-lifo heap-box-drop \
                   vmspace-activate-irq vmspace-activate-irq-control \
                   smp-ap-tasks smp-ap-task-returns smp-data-race \
                   smp-ipi-call smp-tlb-shootdown \
                   cpu-local-large-section \
                   frame-atomic-fallible frame-atomic-uninit \
                   segment-wrapped-size \
                   ram-size-small \
                   mmio-floating-bus \
                   dyn-cpu-local \
                   dma-immutable-static \
                   iomem-podonce-size \
                   task-stack-reuse \
                   kernel-task-entry-private
.PHONY: asterinas-test-all
asterinas-test-all: install install-osdk
	set -eu; \
	for test in $(ASTERINAS_TESTS); do \
		echo "Running test: $${test}"; \
		if [ "$${test}" = irq-rcu-uaf ]; then \
			$(RUN_IRQ_RCU_UAF_TEST); \
		elif [ "$${test}" = vmspace-activate-irq ]; then \
			$(RUN_VMSPACE_ACTIVATE_IRQ_TEST); \
		elif [ "$${test}" = vmspace-activate-irq-control ]; then \
			($(RUN_VMSPACE_ACTIVATE_IRQ_CONTROL_TEST)); \
		elif [ "$${test}" = smp-ap-tasks ]; then \
			($(RUN_SMP_AP_TASKS_TEST)); \
		elif [ "$${test}" = smp-ap-task-returns ]; then \
			($(RUN_SMP_AP_TASK_RETURNS_TEST)); \
		elif [ "$${test}" = smp-ipi-call ]; then \
			($(RUN_SMP_IPI_CALL_TEST)); \
		elif [ "$${test}" = smp-tlb-shootdown ]; then \
			($(RUN_SMP_TLB_SHOOTDOWN_TEST)); \
		elif [ "$${test}" = smp-data-race ]; then \
			$(RUN_SMP_DATA_RACE_TEST); \
		elif [ "$${test}" = cpu-local-large-section ]; then \
			$(RUN_CPU_LOCAL_LARGE_SECTION_TEST); \
		elif [ "$${test}" = frame-atomic-uninit ]; then \
			$(RUN_FRAME_ATOMIC_UNINIT_TEST); \
		elif [ "$${test}" = ram-size-small ]; then \
			$(RUN_RAM_SIZE_SMALL_TEST); \
		elif [ "$${test}" = dma-immutable-static ]; then \
			$(RUN_DMA_IMMUTABLE_STATIC_TEST); \
		elif [ "$${test}" = iomem-podonce-size ]; then \
			$(RUN_IOMEM_PODONCE_SIZE_TEST); \
		elif [ "$${test}" = kernel-task-entry-private ]; then \
			$(RUN_KERNEL_TASK_ENTRY_PRIVATE_TEST); \
		else \
			(cd $(PROJ)/tests/$${test} && $(ASTERINAS_ENV_VARS) cargo osdk miri test); \
		fi; \
	done
	$(RUN_KTEST_RUNNER_TEST)

.PHONY: asterinas-ktest-runner
asterinas-ktest-runner: install install-osdk
	$(RUN_KTEST_RUNNER_TEST)

TOCK_BOARD := $(PROJ)/tock/boards/qemu_rv64_virt
.PHONY: tock
tock: install
	cd $(TOCK_BOARD) && \
		MIRIFLAGS="-Zkmiri-toml=$(TOCK_BOARD)/kmiri.toml" \
		MIRI_SYSROOT="$(SYSROOT)" \
		cargo miri run --target riscv64imac-unknown-none-elf

MIRI_TEST_NAME := debugger_test
MIRI_TEST_FILE := tests/pass/$(MIRI_TEST_NAME).rs
__KMIRI_DIR_TARGET := $(PROJ)/kmiri/target
ANALYSIS_DIR := $(__KMIRI_DIR_TARGET)/analysis
.PHONY: debugger
debugger: install
	export __KMIRI_DIR_TARGET=$(__KMIRI_DIR_TARGET) && \
	export LD_LIBRARY_PATH=$(SYSROOT)/lib && \
    trap 'rm -f $(MIRI_TEST_NAME)' EXIT && \
	rm -rf $(ANALYSIS_DIR) && \
	kmiri-helper $(MIRI_TEST_FILE) --emit=metadata && \
	mv $(ANALYSIS_DIR)/*.json $(__KMIRI_DIR_TARGET)/analysis.json && \
	MIRIFLAGS="$(MIRIFLAGS) --debugger" ./miri run $(MIRI_TEST_FILE)

# This filter can be a substring in the path, e.g. a prefix `no-paging`
# to run all pass and fail tests. The default filter is for all kmiri tests.
MIRI_TEST_FILTER ?= kmiri
BLESS ?=
.PHONY: test
test: install
	./miri test $(if $(BLESS),--bless) $(MIRI_TEST_FILTER)

.PHONY: kmiri-setup
# This generate a precompiled sysroot in `/root/.cache/miri`.
kmiri-setup: install
	export LD_LIBRARY_PATH=$(SYSROOT)/lib/rustlib/x86_64-unknown-linux-gnu/lib && \
	which cargo-miri && \
    cargo miri setup

.PHONY: test-in-blueos
test-in-blueos: kmiri-setup
	export __KMIRI_DIR_TARGET=$(__KMIRI_DIR_TARGET) && \
	export MIRI_SYSROOT="$(MIRI_SYSROOT_REBUILT)" && \
	export LD_LIBRARY_PATH=$(SYSROOT)/lib/rustlib/x86_64-unknown-linux-gnu/lib && \
    trap 'rm -f $(MIRI_TEST_NAME)' EXIT && \
	rm -rf $(ANALYSIS_DIR) && \
	kmiri-helper $(MIRI_TEST) && \
	mv $(ANALYSIS_DIR)/*.json $(__KMIRI_DIR_TARGET)/analysis.json && \
	MIRIFLAGS=--debugger ./miri run $(MIRI_TEST)
