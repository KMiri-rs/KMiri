//! A CPU-local variable used before the CPU-local base register is set must be
//! reported as UB.
//!
//! KMiri starts the base register at its reset value 0, and OSTD's `miri_boot`
//! sets it to `__cpu_local_start`, as `bsp_boot.S` does on x86. This kernel puts
//! the register back to 0 to model an access that happens before boot code set
//! it (the bug pattern of Asterinas #1069): the address becomes `0 + offset`,
//! which no allocation covers.
//!
//! Expected: the first increment succeeds, the second one is reported as
//! "dangling pointer (it has no provenance)". Before KMiri modeled the reset
//! value and the section symbols, both increments landed on the BSP's copy
//! and nothing was reported.
#![no_std]
#![feature(format_args_nl)]

extern crate alloc;

ostd::cpu_local_cell! {
    static COUNTER: u32 = 0;
}

unsafe extern "Rust" {
    fn kern_miri_set_cpu_local_base(base_vaddr: usize);
}

#[ostd::ktest::miri_main]
fn kernel_main() {
    COUNTER.add_assign(1);
    assert_eq!(COUNTER.load(), 1);
    ostd::miri_println!("cpu-local-before-base: access after boot set the base ok");

    // SAFETY: Deliberately undoes what boot code did, to model an access before it.
    unsafe { kern_miri_set_cpu_local_base(0) };
    COUNTER.add_assign(1);
    ostd::miri_println!("cpu-local-before-base: access with base 0 was not reported");
}
