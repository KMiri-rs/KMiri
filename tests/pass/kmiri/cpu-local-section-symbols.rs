//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance

//! KMiri models the CPU-local base register and the `.cpu_local` section the way
//! hardware and the linker do, so that a kernel's own boot code sets the base.
//!
//! - The base register starts at its reset value 0. Before this was modeled, KMiri
//!   preset it to the section start, which hid accesses made before the kernel
//!   set it (Asterinas #1069).
//! - `__cpu_local_start` / `__cpu_local_end` bound the section that holds the
//!   `.cpu_local` statics, as the linker script provides them on hardware.

use std::sync::atomic::{AtomicU32, Ordering};

#[unsafe(link_section = ".cpu_local")]
static FIRST: AtomicU32 = AtomicU32::new(1);
#[unsafe(link_section = ".cpu_local")]
static SECOND: AtomicU32 = AtomicU32::new(2);

unsafe extern "Rust" {
    fn kern_miri_get_cpu_local_base() -> usize;
    fn kern_miri_set_cpu_local_base(base_vaddr: usize);
}

unsafe extern "C" {
    fn __cpu_local_start();
    fn __cpu_local_end();
}

fn main() {
    let start = __cpu_local_start as *const () as usize;
    let end = __cpu_local_end as *const () as usize;
    assert!(start < end);

    for var in [&FIRST, &SECOND] {
        let addr = var as *const AtomicU32 as usize;
        assert!(start <= addr && addr + size_of::<AtomicU32>() <= end);
    }

    unsafe {
        assert_eq!(kern_miri_get_cpu_local_base(), 0);
        kern_miri_set_cpu_local_base(start);
        assert_eq!(kern_miri_get_cpu_local_base(), start);
    }

    // Base + offset reaches the variable once the base is set.
    let offset = &SECOND as *const AtomicU32 as usize - start;
    let local = unsafe { &*((kern_miri_get_cpu_local_base() + offset) as *const AtomicU32) };
    assert_eq!(local.load(Ordering::Relaxed), 2);
}
