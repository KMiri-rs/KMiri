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
    unsafe { kern_miri_start_cpu(cpu, func, arg, 0x7000000 + cpu * 0x200000, 0x100000) }
}
