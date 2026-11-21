//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2-paging.toml -Zmiri-permissive-provenance
//! A slot of a typed page exists from the moment the page is typed. CPU 0
//! publishes the page, and only then is the first to touch a slot. CPU 1, which
//! synchronized with the publication but not with that touch, may read the slot.

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

static PUBLISHED: AtomicBool = AtomicBool::new(false);
static TOUCHED: AtomicBool = AtomicBool::new(false);
static DONE: AtomicBool = AtomicBool::new(false);

fn slot() -> &'static AtomicUsize {
    unsafe { AtomicUsize::from_ptr(ptr::with_exposed_provenance_mut(LINEAR + PAGE)) }
}

fn entry(_: usize) {
    while !PUBLISHED.load(Ordering::Acquire) {
        spin_loop();
    }
    while !TOUCHED.load(Ordering::Relaxed) {
        spin_loop();
    }
    assert_eq!(slot().load(Ordering::Acquire), 0);
    DONE.store(true, Ordering::Release);
}

fn main() {
    unsafe { prepare_paging() };
    smp::start_with_stack(1, entry, 0, CPU1_STACK_END);
    unsafe {
        kern_miri_alloc_pages(PAGE, 1);
        kern_miri_zero(PAGE, 1);
        kern_miri_retype_pages(PAGE, 1, TypedKind::PageTable, 8);
    }
    PUBLISHED.store(true, Ordering::Release);
    assert_eq!(slot().load(Ordering::Acquire), 0);
    TOUCHED.store(true, Ordering::Relaxed);
    while !DONE.load(Ordering::Acquire) {
        spin_loop();
    }
}
