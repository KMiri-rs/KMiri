//! KMiri CPU shims shared by the multi-CPU regression tests.

#![allow(dead_code)]

unsafe extern "Rust" {
    fn kern_miri_cpu_count() -> usize;
    fn kern_miri_current_cpu() -> usize;
    fn kern_miri_start_cpu(
        cpu: usize,
        func: fn(usize),
        arg: usize,
        stack_end: usize,
        stack_size: usize,
    );
}

pub fn count() -> usize {
    unsafe { kern_miri_cpu_count() }
}

pub fn current() -> usize {
    unsafe { kern_miri_current_cpu() }
}

/// Starts `cpu` at `func(arg)` on a stack reserved for that CPU.
pub fn start(cpu: usize, func: fn(usize), arg: usize) {
    start_with_stack(cpu, func, arg, 0x7000000 + cpu * 0x200000)
}

/// Starts `cpu` at `func(arg)` on the 1 MiB stack that ends at `stack_end`. With
/// paging, the stack must be mapped.
pub fn start_with_stack(cpu: usize, func: fn(usize), arg: usize, stack_end: usize) {
    unsafe { kern_miri_start_cpu(cpu, func, arg, stack_end, 0x100000) }
}

unsafe extern "Rust" {
    fn kern_miri_alloc_pages(paddr: usize, count: usize);
    fn kern_miri_copy_cpu_local_section(dst: usize);
}

unsafe extern "C" {
    fn __cpu_local_start();
    fn __cpu_local_end();
}

/// Start and length of the CPU-local section.
pub fn section() -> (usize, usize) {
    let start = __cpu_local_start as *const () as usize;
    (start, __cpu_local_end as *const () as usize - start)
}

/// Allocates pages at physical address `paddr` and copies the CPU-local section
/// there through its virtual address `vaddr`.
pub fn copy_section(paddr: usize, vaddr: usize) {
    let pages = section().1.div_ceil(4096);
    unsafe {
        kern_miri_alloc_pages(paddr, pages);
        kern_miri_copy_cpu_local_section(vaddr);
    }
}

/// `var`'s copy in the copy of the section at `base`.
pub fn local<T>(base: usize, var: &'static T) -> *mut T {
    (base + (var as *const T as usize - section().0)) as *mut T
}
