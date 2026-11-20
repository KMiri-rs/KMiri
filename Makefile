PROJ := $(abspath $(dir $(lastword $(MAKEFILE_LIST)))/..)
SYSROOT := $(shell rustc --print sysroot)
MIRI_SYSROOT_REBUILT := /root/.cache/miri

ASTERINAS_TOML := $(PROJ)/tests/asterinas.toml
ASTERINAS_ENV_VARS := OSDK_LOCAL_DEV=1 MIRIFLAGS="-Zkmiri-toml=$(ASTERINAS_TOML) -Zmiri-permissive-provenance"

define RUN_IRQ_DEADLOCK_TEST
log=$$(mktemp); \
if (cd $(PROJ)/tests/irq-deadlock && $(ASTERINAS_ENV_VARS) cargo osdk miri test) >"$$log" 2>&1; then \
	cat "$$log"; rm -f "$$log"; echo "irq-deadlock unexpectedly completed without aborting"; exit 1; \
fi; \
cat "$$log"; \
if grep -F "IRQ watchdog: possible self-deadlock" "$$log" >/dev/null; then \
	echo "Observed expected IRQ watchdog deadlock diagnostic."; rm -f "$$log"; \
else \
	rm -f "$$log"; echo "irq-deadlock failed without the expected watchdog diagnostic"; exit 1; \
fi
endef

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
VMSPACE_IRQ_ENV_VARS := OSDK_LOCAL_DEV=1 MIRIFLAGS="-Zkmiri-toml=$(PROJ)/tests/vmspace-activate-irq/kmiri.toml -Zmiri-permissive-provenance"

define RUN_VMSPACE_ACTIVATE_IRQ_TEST
log=$$(mktemp); \
if (cd $(PROJ)/tests/vmspace-activate-irq && $(VMSPACE_IRQ_ENV_VARS) cargo osdk miri test) >"$$log" 2>&1; then \
	cat "$$log"; rm -f "$$log"; echo "vmspace-activate-irq unexpectedly completed without aborting"; exit 1; \
fi; \
cat "$$log"; \
if awk '/Undefined Behavior: / && (/has been freed/ || /strongly protected/) { found = 1 } END { exit !found }' "$$log" && grep -F "ostd::mm::VmSpace::activate" "$$log" >/dev/null; then \
	echo "Observed expected Undefined Behavior in VmSpace::activate."; rm -f "$$log"; \
else \
	rm -f "$$log"; echo "vmspace-activate-irq failed without the expected Undefined Behavior in VmSpace::activate"; exit 1; \
fi
endef

define RUN_VMSPACE_ACTIVATE_IRQ_CONTROL_TEST
cd $(PROJ)/tests/vmspace-activate-irq-control && $(VMSPACE_IRQ_ENV_VARS) cargo osdk miri test
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
ifeq ($(ASTERINAS_TEST),irq-deadlock)
asterinas: install install-osdk
	$(RUN_IRQ_DEADLOCK_TEST)

else ifeq ($(ASTERINAS_TEST),irq-rcu-uaf)
asterinas: install install-osdk
	$(RUN_IRQ_RCU_UAF_TEST)

else ifeq ($(ASTERINAS_TEST),vmspace-activate-irq)
asterinas: install install-osdk
	$(RUN_VMSPACE_ACTIVATE_IRQ_TEST)

else ifeq ($(ASTERINAS_TEST),vmspace-activate-irq-control)
asterinas: install install-osdk
	$(RUN_VMSPACE_ACTIVATE_IRQ_CONTROL_TEST)

else
asterinas: install install-osdk
	echo $(ASTERINAS_TEST)
	cd $(PROJ)/tests/$(ASTERINAS_TEST) && \
		$(ASTERINAS_ENV_VARS) cargo osdk miri test
endif

ASTERINAS_TESTS := init frame-realloc page-table-realloc segment-zeroed \
                   task-switch-back task-exit-last task-exit-with-runnable \
                   irq-simulation irq-trap-path irq-deadlock irq-rcu-uaf \
                   irq-rcu-uaf-lifo heap-box-drop \
                   vmspace-activate-irq vmspace-activate-irq-control
.PHONY: asterinas-test-all
asterinas-test-all: install install-osdk
	set -eu; \
	for test in $(ASTERINAS_TESTS); do \
		echo "Running test: $${test}"; \
		if [ "$${test}" = irq-deadlock ]; then \
			$(RUN_IRQ_DEADLOCK_TEST); \
		elif [ "$${test}" = irq-rcu-uaf ]; then \
			$(RUN_IRQ_RCU_UAF_TEST); \
		elif [ "$${test}" = vmspace-activate-irq ]; then \
			$(RUN_VMSPACE_ACTIVATE_IRQ_TEST); \
		elif [ "$${test}" = vmspace-activate-irq-control ]; then \
			($(RUN_VMSPACE_ACTIVATE_IRQ_CONTROL_TEST)); \
		else \
			(cd $(PROJ)/tests/$${test} && $(ASTERINAS_ENV_VARS) cargo osdk miri test); \
		fi; \
	done

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
