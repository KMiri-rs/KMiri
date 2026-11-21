//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2-paging.toml -Zmiri-permissive-provenance
//! CPU 0 types a page, and CPU 1 is the first to touch one of its slots without
//! having synchronized with the typing. The access races with the page's creation
//! even though CPU 1 itself makes the slot's allocation appear.

#[path = "../../../dev/test_utils/smp.rs"]
mod smp;
#[path = "../../../dev/test_utils/physical_copy.rs"]
mod utils;

use std::hint::spin_loop;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use utils::*;

const PAGE: usize = 0x0300_0000;
const CPU1_STACK_END: usize = LINEAR + 0x0620_0000;

static TYPED: AtomicBool = AtomicBool::new(false);

fn entry(_: usize) {
    while !TYPED.load(Ordering::Relaxed) {
        spin_loop();
    }
    let slot = unsafe { AtomicUsize::from_ptr(ptr::with_exposed_provenance_mut(LINEAR + PAGE)) };
    slot.load(Ordering::Acquire); //~ ERROR: Data race detected
}

fn main() {
    unsafe { prepare_paging() };
    smp::start_with_stack(1, entry, 0, CPU1_STACK_END);
    unsafe {
        kern_miri_alloc_pages(PAGE, 1);
        kern_miri_zero(PAGE, 1);
        kern_miri_retype_pages(PAGE, 1, TypedKind::PageTable, 8);
    }
    TYPED.store(true, Ordering::Relaxed);
    loop {
        spin_loop();
    }
}
