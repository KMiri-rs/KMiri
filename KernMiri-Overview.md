# KernMiri: An Overview

Author: [@cchanging](https://github.com/cchanging)

Hackmd：<https://hackmd.io/bLRtlCH0T9udi9OLGrrXuQ>

## Introduction

Demonstrating the soundness of OSTD is challenging for two reasons. First, the Rust language only provides an inexhaustive list of UBs described informally; we cannot prove a property that lacks precise definitions. Second, low-level, machine-oriented unsafe code in the kernel is error-prone and can cause safety problems beyond language design, such as programming errors in the manipulation of page tables.

In theory, the soundness of a Rust system can be verified formally by using deductive-style verification (e.g., Prusti and Verus) or bounded model checking (e.g., Kani). But in practice these approaches would be unsustainable for rapidly-evolving projects like Asterinas, for which the development speed of the verification code can hardly catch up with that of the system code. In addition, verification tools restrict the set of Rust language features available to system developers, which is unacceptable for projects like Asterinas that are written in idiomatic Rust, using advanced Rust features only available from Rust's nightly toolchains.

Instead, we decide to adopt Miri, the official UB detection tool provided by Rust, which is developer friendly and powerful enough to capture real-world UB bugs [??]. Miri does not require kernel developers to have any expertise in formal verification or maintain any formal verification code. Built upon the infrastructure provided by the Rust compiler, Miri supports the idiomatic Rust code and most up-to-date Rust language features. Miri works by compiling Rust code into MIR, executing the MIR by interpretation, and capturing UBs along the way using a set of built-in checkers such as borrow checkers [??] and data race checkers [??]. During this process of interpretation, "all Undefined Behavior that has the potential to affect a program's correctness is being detected by Miri" [??].

Unfortunately, Miri has been designed for Rust applications and thus lacks of support for Rust OS kernels that involve low-level software-hardware interactions. In essence, Miri is an MIR virtual machine plus a shim layer that emulates common target host OS services such as environment variables, threads, files, sockets, and memory mapping. This allows Miri to interpret not only "pure" Rust p (i.e., `no_std`) but also "standard" Rust programs that rely on the Rust and C standard libraries. However, Rust OS kernels are neither "pure" nor "standard"; they are "privileged." Privileged code includes low-level operations that access essential hardware features (e.g., CPU registers and paging) and bootstrap its own execution environment (rather than depending on Libc’s established environment). The "privileged" Rust code, which includes inline assembly, will cause Miri to terminate prematurely or report false alarms.

Existing Rust OS projects [??, ??] work around the limitations of Miri by significantly restricting the kernel code that can be tested, compromising the potential for comprehensive safety verification. Specifically, they have to separate "pure" Rust code from "privileged" one so that only the "pure" code is reachable and tested by Miri. For example, page translation tables may be implemented as if they are an abstract data structure of radix trees and thus testable by Miri [??]. This approach not only puts more restrictions on kernel code organization, but also fails to consider the CPU architecture's impact of page tables, thereby missing potential environment-level UBs due to misconfigured or buggy page tables.

In light of these limitations, we introduce KernMiri, a retrofitted version of Miri that can cover a broader range of code and identify more kinds of UBs in Rust OS projects than the vanilla Miri. At the core of KernMiri is Mirch, a minimalism, pseudo CPU architecture. Mirch takes MIR as its instruction set, supports a simplistic three-level paging scheme, and specifies a straightforward boot protocol. A Rust OS may support Mirch as a new CPU architecture in mostly the same way as it supports real CPU architectures. The differences are that (1) it takes much less enginneering efforts thanks to Mirch's minimalism design and (2) it interacts with the (pseudo-)CPU in Rust, rather than assembly. This means that as long as a Rust OS supports the Mirch architecture, then it can be run as whole by KernMiri.

Compared with real CPU architectures, Mirch provides one unique feature: page state tracking. Mirch requires a compliant OS to invoke special MIR instructions to explicitly convert physical pages to a particular state before accessing them for a particular purpose. The states and their transfers between them are shown in the figure below.

![image](https://hackmd.io/_uploads/rJzAzaPV1g.png)

All memory loads and stores performed via MIR instructions are validated according to the page states for UB detection. The page state tracking, along with the page scheme, allows KernMiri to detect more kinds of UBs than Miri, including environment-level UBs such as buggy or misconfigured page tables (see the table below).

| UB Classes             | UB Examples                                                                                                          | Miri | KernMiri |
| ---------------------- | -------------------------------------------------------------------------------------------------------------------- | ---- | -------- |
| UBs on typed memory    | Use-after-free, buffer overflows, misaligned accesses, uninitialized accesses, mutation of immutable, and data races | ✓    | ✓        |
| UBs on page tables     | A typed memory page being mapped to the user space or having more than one active kernel virtual address             | ❌    | ✓        |
| UBs on untyped memory  | Use-after-free, buffer overflows, misaligned access, non-volatile accesses, and non-POD accesses                     | ❌    | ✓        |
| UBs on page management | Access to reserved or free physical memory pages                                                                     | ❌    | ✓        |


OSTD has been extended to support Mirch as a new CPU architecture, alongside x86-64 and RISC-V. The codebase currently includes over 340 `unsafe` blocks (`unsafe { ... }`), with approximately 220 of these (65%) found in the CPU architecture-independent portions of the codebase (i.e., outside the `arch/` directory). Within this architecture-independent code, memory management (`mm/`) is the biggest `unsafe` user among all top-level modules, account for around 150 `unsafe` blocks (40%). At the secondary module level, the page table module (`mm/page_table/`) leads with 72 `unsafe` blocks (20%). To ensure robustness, these modules are already supported by a comprehensive set of unit tests, routinely executed by our CI system on real hardware. With the addition of Mirch support, these tests are now reusable and executable by KernMiri, providing an additional layer of verification to detect potential UBs.

- [ ] unwind safety?
- [ ] ref cnt overflow (`Page`)?

![image](https://hackmd.io/_uploads/r1Ga_CPVke.png)

The remaining of this documentation provides the design, implementation, and evaluation of KernMiri.

## Design

### Memory System
Miri's original memory system is straightforward, only supports managing memory item explicitly allocated by the Rust runtime, such as stack, static variables, and heap objects allocated via `__rust_alloc`. Each item is stored as an `Allocation`, which contains its corresponding bytes and  will be assigned an address by Miri. The assignment of addresses follows a simple linear growth pattern, ensuring only that allocations do not overlap with each other. There are no strict requirements on the absolute addresses, as Miri assumes that the programs it supports should not directly access memory through absolute addresses.

Unlike general Rust programs, OS code will directly manage and utilize memory regions through pointers, creating memory objects directly on specific physical pages and accessing them directly through addresses, which is necessary and not compatible with Miri's allocation management. To avoid overly intrusive modifications to Miri's execution engine, KernMiri aims to overload Miri's address management module and introduce new functionalities for allocating `Allocation`s without disrupting the existing management of `Allocation`s. These enhancements are designed to achieve precise address management and provide additional capabilities for allocating memory items directly managed by the OS.

The new memory system reserves a memory region as **pseudo-physical memory** in Miri and manages it at the page level. Based on the pseudo-physical memory, KernMiri performs a more strict address assignment: 
- Objects allocated by the Rust runtime in the OS reside in the kernel code segment, including stack and static variables. When allocating addresses for these objects, their addresses must be precisely assigned to their designated locations. Since the internal structure of the kernel code segment is typically treated as a black box by the OS, the internal addresses can be linearly incremented without strict requirements. 
- Other memory objects used by the OS are managed by the OS itself at the page level. For example, a page might be allocated to store a pagetable node or a slab of a specific size for a slab allocator. Therefore, KernMiri provides the ability to create `Allocation`s on a page when the OS converts it into typed memory and assign them addresses based on the page's address.

For the access operations of allocations, KernMiri retains Miri's original execution logic. This ensures compatibility with other modules and preserves the existing UB checking mechanisms.
![KernMiri_memory](https://hackmd.io/_uploads/HJowxhRoyg.png)



### PageTable
Modern OS universally support paging systems. As a result, Miri's original virtual address management is insufficient to support certain memory access behaviors in OS environments. To address this, KernMiri introduces an additional abstraction for `PageTable` and provides corresponding interaction interfaces. These interfaces allow the OS to interact with KernMiri's `PageTable` in a manner similar to how it would interact with hardware, such as retrieving or setting the root physical address of the currently active `PageTable`.

Typically, a Rust OS initializes a boot page table using assembly code during the early boot phase and activates the paging system. Later, during the Rust code initialization phase, it generates a new, complete page table. To emulate this behavior, KernMiri initializes a boot page table in pseudo-physical memory before starting interpretation. It then sets this boot page table as the currently active page table, effectively simulating the functionality of the assembly code that cannot be interpreted.

With this approach, KernMiri can always walk the currently active page table during the interpretation of OS memory accesses to locate the corresponding physical address. This enables subsequent allocation lookups and ensures accurate emulation of OS memory management behavior.

### Thread Management
Threads are a type of resource managed by the OS. When interpreting pure Rust programs, Miri cannot understand the OS's thread management operations through Rust code alone. To address this, Miri internally maintains a simulated thread management system. It uses additional shims to identify thread-related operations from the underlying OS (e.g., Unix/Windows systems) and translates them into corresponding actions within its internal thread management system to simulate thread behavior. However, these shims are limited to supporting only common Unix/Windows systems.

KernMiri, designed specifically for new Rust OSes, cannot follow the same approach as Miri by implementing shims for each target OS. Instead, it provides a more generic set of shims. These shims allow Rust OSes to interact with KernMiri, informing it of thread-related actions such as creating a thread or switching from one thread to another. This enables KernMiri's internal thread management system to function correctly in the context of the OS.

Notably, OSes typically manage the stack space for each thread independently, allocating physical pages for them. The stack space of each thread must not overlap with the kernel code segment's stack segment. To address this, KernMiri associates a dedicated stack space with each thread in its thread management system. When interpreting execution for a specific thread, the address allocation logic for stack variables uses the corresponding thread's stack space.

### SMP and CPU-Local Variables

KernMiri incorporates support for SMP-related functionalities in its simulation framework. This design decision is motivated by two key considerations: (1) operating systems frequently implement multicore-specific logic and utilize CPU-local variables, and (2) SMP simulation can compensate for lacking of the original thread random switching mechanism and its race UB detection abilities.

The implementation of CPU-local mechanisms in the kernel typically requires utilizing the `.cpu_local` section to store relevant variables for subsequent specialized operations. However, since Miri's interpretation occurs prior to the linking phase, it cannot determine the actual address of the `.cpu_local` section. To address this, KernMiri reserves a dedicated memory region within the pseudo-physical memory's kernel code area specifically for storing CPU-local variables, effectively emulating the functionality of the `.cpu_local` section. During global static variable identification, when KernMiri detects code directing a variable to be linked to the `.cpu_local` section, it will turn to allocate the variable to this predefined memory region.

While the original Miri implementation relied on random thread scheduling to trigger potential race conditions, KernMiri disables this functionality. Instead, with multicore support introduced, random scheduling across multiple cores achieves the same race condition detection capability. 
![image](https://hackmd.io/_uploads/SyAJjLkgxl.png)

As illustrated in the figure above, each CPU core is bound to a specific thread at any given time. Thread switching on the same core only occurs when the interpreter executes scheduler-related code. However, during execution, KernMiri probabilistically performs random context switches between different CPU cores, thereby maintaining the equivalent effect of the original thread randomization approach.

## Implementation

We have a preliminary implementation at https://github.com/cchanging/miri/tree/kernmiri. In the following section we will provide an overview for its implementation.

### Pseudo Physical Memory Configuration

First, we provide an overview of the pseudo-physical memory maintained by KernMiri. As previously mentioned, KernMiri pre-allocates a contiguous memory region to serve as pseudo-physical memory before beginning interpretation. This memory space is partitioned and managed according to the `PhysConfig` structure, as illustrated in the following code comments:

```rust=
/// Physical Memory Configuration.
/// 
/// Represents the layout of physical memory in the system with the following structure:
/// 
/// |<-------------------------------Kernel Code Section-------------------------------->|
/// |<-Boot PT->|<-Kernel Static Section->|<-CPU-local Section->|<-Kernel Stack Section->|
/// |-----------|------------------------------------------------------------------------|
/// 0x0      0x10000                                                             `kernel_code_size`
/// 
/// |<-Kernel Code Section->|<-----Free Pages------>|
/// |-----------------------|-----------------------|
/// 0x0              `kernel_code_size`        `total_mem_size`
///
pub struct PhysConfig {
    /// Total physical memory size of pseudo physical memory in bytes
    pub total_mem_size: usize,
    /// Size of kernel code section in bytes (includes boot page table, static data, etc.)
    pub kernel_code_size: usize,
    /// Size of kernel static data section in bytes (after boot page table)
    pub kernel_static_size: usize,
    /// Size per-CPU local storage area in bytes
    pub cpu_local_size: usize,
    ...
}
```

Here, all allocations automatically managed by the Rust runtime are placed by KernMiri within the kernel code section. These allocations are further organized into distinct subsections based on their types. The memory partitioning follows predefined parameters, but users may override them via command-line arguments. Note that the configuration must adhere to certain invariants (e.g., `kernel_code_size` cannot exceed `total_mem_size`).

### Paging Configuration for Mirch Architecture

As a CPU architecture, Mirch needs to define some hardware attributes, primarily providing definitions related to the paging system:

- **Page Size**: `0x1000`
- **Number of Page Table Levels**: `4`
- **Page Table Entry Size**: `8`
- **Page Table Entry Flags**:
  - `bit0`: Valid
  - `bit1`: Readable
  - `bit2`: Writable
  - `bit7`: Huge
  - `bit3-bit6`: Reserved bits, which the OS can define. Additional definitions only serve to check value matches during interpretation and do not affect actual page table behavior.

In addition, there are two settings unrelated to the architecture but relevant to the paging functionality:

- `kernel_code_base_vaddr`: Specifies the virtual address to which the kernel code section should be mapped. KernMiri assumes that the kernel code section has a fixed mapping. However, since it cannot obtain information from the linking stage, the virtual address must be explicitly specified. This mapping must be initialized in the boot page table; otherwise, early static and stack variable allocations will not function properly with the page table.

- `boot_pt_linear_mapping_base_vaddr`: Indicates the virtual address for the linear mapping provided by the boot page table. Kernels often use a simple linear mapping during the boot phase for initialization. Hence, KernMiri also provides this feature.

KernMiri will map 1 GB of the kernel code section and linear mapping to address 0 in the Boot Page Table based on the two base addresses above. Therefore, these addresses must be aligned to 1 GB.

### Interaction Between Pseudo Physical Memory and the Miri Memory Allocation System

With the introduction of **pseudo-physical memory**, KernMiri significantly refactored the memory and address allocation management logic in `/src/alloc_address/mod.rs` to bind its original memory allocation and address management behavior to the reserved physical memory. The new system implements the following additional features compared to the original one:

- **Additional Allocation Pathways**: Through the provided shims API, a specified physical page can be converted into typed memory, allowing the creation of `Allocation`s for typed objects in pseudo-physical memory.

- **Physical Address Consistency**: All `Allocation`s stored in the address management system now have physical addresses that actually map to the pseudo-physical memory. This means their **backed bytes** correspond to the content at the respective addresses in the reserved physical memory. Notably, KernMiri enforces this property with the following mechanism:
  - Any `Allocation` originally created by the Rust runtime (e.g., stack variables, `static` variables) will have their **backed bytes** migrated to pseudo-physical memory upon first access. This ensures that memory operations during interpreted execution reference the correct addresses. Without this mechanism, operations like copying a stack-allocated array via `ptr::copy` would be misinterpreted.

- **Stack Deallocation on Exit**: Stack allocations are now destroyed when the stack unwinds. Since the available memory space is now constrained, failing to deallocate stack memory would quickly lead to address exhaustion.


### Shims API List

```rust=
//! Page Management

/// Notifies KernMiri to allocate `count` pages at address `paddr`.
///
/// The kernel should not allocate pages at the same address repeatedly.
fn kern_miri_alloc_pages(paddr: usize, count: usize);

/// Notifies KernMiri to deallocate `count` pages at address `paddr`.
///
/// The kernel can only deallocate addresses allocated via `kern_miri_alloc_pages`.
fn kern_miri_dealloc_pages(paddr: usize, count: usize);

/// Notifies KernMiri to type `count` pages at address `paddr` to `page_type`.
///
/// The kernel can only retype allocated pages.
fn kern_miri_type_pages(paddr: usize, count: usize, page_type: PageType);
```
```rust=
//! Read/Write Operations for Untyped Pages

fn kern_miri_read_u8_untyped(ptr: *const u8) -> u8;
fn kern_miri_write_u8_untyped(ptr: *mut u8, value: u8);
fn kern_miri_read_u16_untyped(ptr: *const u16) -> u16;
fn kern_miri_write_u16_untyped(ptr: *mut u16, value: u16);
fn kern_miri_read_u32_untyped(ptr: *const u32) -> u32;
fn kern_miri_write_u32_untyped(ptr: *mut u32, value: u32);
fn kern_miri_read_u64_untyped(ptr: *const u64) -> u64;
fn kern_miri_write_u64_untyped(ptr: *mut u64, value: u64);
fn kern_miri_copy_untyped(dst: *const u8, src: *const u8, len: usize);
```
```rust=
//! Page Table Management

/// Activates the page table rooted at physical address `root_paddr` as the current page table.
fn kern_miri_set_root_page_table(root_paddr: usize);

/// Retrieves the root physical address of the currently active page table.
fn kern_miri_get_root_page_table() -> usize;
```
```rust=
//! Thread Management

/// Creates a new thread executing `func` with the stack top at `stack_end_vaddr` and size `stack_size`.
fn kern_miri_create_new_thread(func: fn(), thread_id: usize, stack_end_vaddr: usize, stack_size: usize);

/// Switches to the thread with ID `thread_id`.
fn kern_miri_switch_to(thread_id: usize);

/// Initializes a simulated CPU with ID `cpu_id`, creating and running a thread with ID `thread_id` executing `func`.
fn kern_miri_ap_init(cpu_id: usize, func: fn(), thread_id: usize, stack_end_vaddr: usize, stack_size: usize);
```

## Evaluation
Raceos（https://github.com/arceos-org/arceos ）

## Limitations and Discussions

While KernMiri is a valuable tool for improving confidence in soundness, it does not provide an absolute guarantee. Firstly, Miri lacks knowledge of real CPU architectures, making it unable to detect bugs in assembly code. Secondly, KernMiri is not a machine emulator like QEMU, and therefore, it does not emulate peripheral devices. This limitation means that kernel-mode tests requiring device interaction cannot be executed using KernMiri. Finally, both Miri and KernMiri can only detect undefined behavior (UB) if it is triggered during a specific execution. To address this limitation, we plan to integrate KernMiri with fuzzing techniques to diversify test cases and uncover previously unknown UBs.

## Related Work

Static Analysis for OSes (pattern-driven)

Fuzzing

Static Analysis for OSes
* Sanitizers (RustSan)

Formal verification



