# KMiri

KMiri is a Miri-based execution and checking environment for Rust kernels. It lets kernel code run under Miri's MIR interpreter while adding the kernel-specific execution model that ordinary Miri does not provide.

The repository has two main parts:

- **kmiri**: the Miri fork and kernel execution/checking support.
- **MIR debugger**: an interactive debugger for inspecting a running kmiri/Miri session at the MIR/interpreter level.

## kmiri

kmiri focuses on making kernel memory-management and scheduling code executable under Miri without reducing it to ordinary user-space Rust. Its core functionality is summarized below.

### Pseudo Physical Memory

kmiri reserves a pseudo-physical memory region and manages it at page granularity. Runtime-managed allocations such as stacks and statics are placed in the kernel-code area, while OS-managed memory can be represented as allocations backed by concrete physical pages.

This lets kernel code that creates objects at specific physical addresses still use Miri's allocation, provenance, initialization, bounds, and lifetime checks.

### Page State Tracking

kmiri tracks physical page states and validates memory accesses against those states. The kernel tells kmiri when pages are allocated, deallocated, or converted to a specific use.

This is the mechanism used to catch kernel-level memory errors that ordinary Miri does not see, including:

- Accessing free or reserved physical pages.
- Using untyped pages through ordinary typed memory operations.
- Performing invalid untyped memory reads, writes, or copies.
- Reusing or freeing pages in ways that violate the page-state model.

### Page Table Model

kmiri models a paging architecture and maintains the currently active page table. Kernel code can set or query the root page table, and kmiri walks the active page table while interpreting memory accesses.

This makes page-table bugs visible during interpretation, including invalid mappings, stale mappings, incorrect page-table updates, and cases where one physical page is exposed through mappings that violate the kernel's memory-safety assumptions.

### Address-Aware Allocation

kmiri binds Miri allocations to addresses inside pseudo-physical memory. Allocations created by the Rust runtime are migrated into the pseudo-physical memory backing store when needed, and kernel-created typed memory can be allocated directly on a page.

This keeps Miri's normal UB checks useful even when the kernel manipulates memory through explicit addresses, copies bytes between address ranges, or builds typed objects on OS-managed pages.

### Kernel Thread Model

kmiri provides shims for kernel-managed threads. The kernel can notify kmiri when it creates a thread, switches to a thread, or initializes an application processor with a thread entry point and stack.

Each simulated thread has its own stack region, so stack allocations are interpreted in the address space that the kernel scheduler expects.

### SMP and CPU-Local Data

kmiri models multiple CPUs and CPU-local storage. CPU-local variables are placed in a reserved pseudo-physical memory area, and simulated CPUs can be bound to kernel threads.

Random switching across simulated CPUs provides a way to exercise race-prone paths while preserving the kernel's explicit scheduler behavior on each CPU.

### Kernel Shim APIs

kmiri exposes shim APIs that kernel code uses to synchronize its low-level operations with the interpreter model:

- Page management: allocate, deallocate, and type physical pages.
- Untyped memory operations: read, write, and copy untyped memory through explicit helper calls.
- Page table management: set and get the active root page table.
- Thread and CPU management: create threads, switch threads, and initialize simulated CPUs.

These APIs are intentionally small: they give kmiri the events it needs to maintain a faithful memory and execution model without emulating a complete machine.

### What kmiri Checks

kmiri keeps Miri's ordinary Rust UB checks and extends their reach into kernel execution:

- Use-after-free and dangling pointer/reference use.
- Out-of-bounds and misaligned memory accesses.
- Uninitialized reads.
- Invalid pointer provenance.
- Invalid aliasing under Miri's borrow models.
- Data races when the configured checker can observe them.
- Page-table and mapping mistakes that expose typed memory unsafely.
- Invalid access to untyped, free, or reserved physical pages.
- Broken ownership, initialization, and cleanup invariants in OSTD/Asterinas memory abstractions.

kmiri is an execution-time checker. It reports bugs only on paths that are actually interpreted.

## Running

Install the local Miri tool and run the target kernel tests through OSDK integration:

```bash
cd kmiri
./miri install --debug
cd ../tests/init
OSDK_LOCAL_DEV=1 cargo osdk miri test
```

The exact command can vary with the selected Asterinas, OSDK, Rust nightly, and kmiri branches. Use the versions pinned by the current workspace.

## MIR Debugger

![](https://github.com/user-attachments/assets/825d7ee5-f11a-46fd-9f60-469d35a23b84)

![](https://github.com/user-attachments/assets/db27e754-0a9c-4fb4-8e48-98260783d8ad)

The MIR debugger is a TUI debugger built into kmiri. It is enabled by passing `--debugger` to Miri and is used to inspect interpreter snapshots, move through MIR execution, and run until selected targets such as a function instance, source line, or MIR terminator.

For example:

```bash
./miri run tests/pass/debugger_test.rs --debugger
```

When running through OSDK, pass the same flag through `MIRIFLAGS`:

```bash
OSDK_LOCAL_DEV=1 MIRIFLAGS="--debugger" cargo osdk miri test
```

OSDK also has a dedicated `miri-debugger` subcommand to invoke the debugger.

Full commands for debugging `tests/init` via the TUI:

```bash
./tool/install_miri_osdk.sh

cd tests/init
OSDK_LOCAL_DEV=1 MIRIFLAGS="--remap-path-prefix=$(rustc --print=sysroot)/lib/rustlib/src/rust/library/= --remap-path-prefix=$(realpath ../../asterinas)/=" cargo osdk miri-debugger
```

### Panes

The TUI is split into panes. Focused panes have highlighted borders, and `Tab` / `Shift+Tab` moves focus through the main panes.

| Panel | Shows |
| --- | --- |
| MIR | Current basic-block CFG summary and the MIR statements/terminator for the current location. The active MIR line is highlighted; terminators are styled separately. |
| Stack | Current thread stack frames, function names, source locations, selected frame, current thread ID, and stack-pointer metadata when available. |
| Source | Source snippet for the current MIR location, with the current source range highlighted and the file/line range in the title. |
| Locals | Locals for the selected stack frame. Columns include local ID, source name, type, and rendered value; values are styled by state such as dead, uninitialized, pointer, or initialized. |
| Allocations | Miri allocations. Columns include `AllocID`, base physical address, deallocation state, memory kind, size, alignment, exposed provenance, global name, and related locals. |
| Output | Captured stdout/stderr from the interpreted program while the debugger is active. |
| Instances | Searchable function/MIR instances with source file and line range. This pane is also used to run to an instance or to a source line. |
| Status bar | Current run mode, step count, thread ID, focused pane, history usage, record mode, instance-search state, run target, and context-sensitive key help. |
| Borrow Stacks | Modal pane opened with `s`. Shows Stacked Borrows or Tree Borrows state for allocations and, for a selected borrow item, the source span associated with that tag when available. |

### Shortcuts

| Key | Meaning |
| --- | --- |
| `q` | Quit the debugger. |
| `n` | Step over by one debugger snapshot. A numeric prefix repeats the step, e.g. `10n`. |
| `Space` | Step to the current frame's next MIR terminator. A numeric prefix repeats terminator stops. |
| `b` | Step back through recorded debugger history. A numeric prefix moves back multiple snapshots. |
| `c` | Continue execution without stopping at intermediate debugger snapshots. |
| `e` | Run to program end. |
| `t` | Run to a MIR terminator. A numeric prefix skips multiple terminator hits. |
| `/` | Focus Instances and enter instance search. |
| `?` | Focus Instances, clear the current query, and enter search. |
| `Enter` in Instances | Run to the selected instance. If the query contains `.rs`, treat it as a source target such as `path/to/file.rs` or `path/to/file.rs:line`. |
| `.` / `,` in Instances | Move to next / previous search match. |
| `Esc` in Instances | Clear the current instance search query. |
| `[` / `]` | Scroll the status-bar command/help text horizontally; while editing instance search, move to previous / next search-history entry. |
| `S` | Toggle recording all intermediate debugger states. |
| `F` | Toggle freeze mode. When frozen, MIR and Source panes stay centered on the highlighted location. |
| `D` | Toggle display of dead allocations/locals-related dead data in allocation and borrow views. |
| `s` | Toggle the Borrow Stacks modal. |
| `Tab` / `Shift+Tab` | Move focus to the next / previous main pane. |
| `Up` / `Down` | Navigate or scroll within the focused pane. In Stack and Instances, this changes selection. |
| `PageUp` / `PageDown` | Page-scroll the focused pane or page the Instances selection. |
| `Left` / `Right` | Horizontal scroll in the focused pane. |
| Mouse scroll / click | Focus the hovered pane and scroll it; clicking inside Borrow Stacks selects a row. |

Numeric prefixes are collected before execution/navigation keys that consume counts, notably `n`, `Space`, `b`, and `t`.

### Execution Modes

- **Step over**: `n` sends `StepOver`; it advances by debugger snapshots. With history replay active, `n` first moves forward through recorded snapshots before asking the interpreter for more steps.
- **Step frame terminator**: `Space` runs until the current frame reaches a MIR terminator. If execution enters callees, it waits until control returns to the anchored frame before stopping.
- **Run to terminator**: `t` runs until a MIR terminator is reached, optionally skipping multiple terminator hits with a numeric prefix.
- **Run to instance**: search or select an item in Instances, then press `Enter`; execution stops when the active frame's function instance matches the selected target.
- **Run to source line**: type a source target such as `path/to/file.rs` or `path/to/file.rs:line` in Instances and press `Enter`; execution stops when the current frame matches that file, and the line if one was provided.
- **Continue**: `c` switches to continuous execution and suppresses intermediate debugger snapshots.
- **Run to end**: `e` continues until program completion.
- **Reverse view**: `b` moves backward through recorded debugger snapshots. This is history navigation in the TUI, not reverse execution in the interpreter.

### Break And Stop Targets

The TUI debugger currently uses run-to targets rather than a persistent breakpoint table. Supported stop targets are:

- **Function/MIR instance**: use Instances search (`/` or `?`) and press `Enter` on the selected instance.
- **Source line**: enter a source target such as `path/to/file.rs` or `path/to/file.rs:line` in the Instances search box and press `Enter`.
- **MIR terminator**: press `t` to stop at a terminator, or `Space` to stop at a terminator in the current frame.
- **Program end**: press `e`.
- **Interpreter failure**: UB reports, panics, unsupported operations, and abnormal interpreter termination are reported by kmiri/Miri's normal diagnostic path. The TUI does not provide a separate persistent failure-state store.

## Current Status

kmiri is experimental and tied closely to Rust nightly, Miri internals, Asterinas, and OSDK. Treat failures as something to classify:

- A real bug in the checked kernel code.
- A missing or inaccurate kmiri model.
- A current limitation of Miri.

kmiri is useful as a development-time checker for kernel unsafe abstractions. It is not a complete machine emulator, does not execute assembly, does not emulate devices like QEMU, and does not prove paths that were not run.
