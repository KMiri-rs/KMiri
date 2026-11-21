//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2-paging.toml -Zmiri-permissive-provenance
//! A copy of the CPU-local section is ordinary memory for race detection: CPU 0
//! writes CPU 1's copy of a variable while CPU 1 writes it, with nothing
//! ordering the two writes.

#[path = "../../../dev/test_utils/smp.rs"]
mod smp;
#[path = "../../../dev/test_utils/physical_copy.rs"]
mod utils;

use std::cell::UnsafeCell;
use std::hint::spin_loop;
use std::sync::atomic::{AtomicBool, Ordering};

#[repr(transparent)]
struct Plain(UnsafeCell<usize>);
unsafe impl Sync for Plain {}

#[unsafe(link_section = ".cpu_local")]
static X: Plain = Plain(UnsafeCell::new(0));
static DONE: AtomicBool = AtomicBool::new(false);

/// `var`'s value in the copy of the section at `base`.
fn at(base: usize, var: &'static Plain) -> *mut usize {
    smp::local(base, var).cast()
}
/// Physical address of the copy, and its virtual address.
const COPY_PADDR: usize = 0x0200_0000;
const COPY: usize = utils::LINEAR + COPY_PADDR;
/// CPU 1 runs on a stack in the mapped window.
const CPU1_STACK_END: usize = utils::LINEAR + 0x0620_0000;

fn entry(base: usize) {
    unsafe { *at(base, &X) = 1 };
    DONE.store(true, Ordering::Relaxed);
}

fn main() {
    unsafe { utils::prepare_paging() };
    smp::copy_section(COPY_PADDR, COPY);
    smp::start_with_stack(1, entry, COPY, CPU1_STACK_END);
    while !DONE.load(Ordering::Relaxed) {
        spin_loop();
    }
    unsafe { *at(COPY, &X) = 2 }; //~ ERROR: Data race detected
}
