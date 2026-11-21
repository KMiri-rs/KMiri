//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance
//! In a copy of the CPU-local section, only the variables' copies are memory: an
//! address that no variable covers in the section is dangling in the copy too.

#[path = "../../../dev/test_utils/smp.rs"]
mod smp;
#[path = "../../../dev/test_utils/physical_copy.rs"]
mod utils;

use std::sync::atomic::{AtomicUsize, Ordering};

#[unsafe(link_section = ".cpu_local")]
static X: AtomicUsize = AtomicUsize::new(7);
/// Physical address of the copy, and its virtual address.
const COPY_PADDR: usize = 0x0200_0000;
const COPY: usize = utils::LINEAR + COPY_PADDR;

fn main() {
    let x = smp::local(0, &X) as usize; // offset of X in the section
    let (_, len) = smp::section();
    unsafe { utils::prepare_paging() };
    smp::copy_section(COPY_PADDR, COPY);
    assert_eq!(unsafe { &*smp::local(COPY, &X) }.load(Ordering::Relaxed), 7);
    // The last byte of the section's copy: no variable lies there.
    assert!(x + 8 < len);
    let _ = unsafe { *((COPY + len - 1) as *const u8) }; //~ ERROR: dangling pointer
}
