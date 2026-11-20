//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance -Zmiri-disable-stacked-borrows

//! With the borrow tracker disabled, KMiri still creates the allocations it
//! makes for kernel memory on first use: a typed slot in a retyped page, and a
//! `.cpu_local` static whose address is taken.

use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

#[path = "../../../dev/test_utils/physical_copy.rs"]
mod utils;

use utils::*;

#[unsafe(link_section = ".cpu_local")]
static LOCAL: AtomicU32 = AtomicU32::new(7);

fn main() {
    unsafe {
        prepare_paging();
        kern_miri_alloc_pages(DST, 1);
        kern_miri_retype_pages(DST, 1, TypedKind::Slab, 8);
        let slot = ptr::with_exposed_provenance_mut::<u64>(LINEAR + DST);
        slot.write(42);
        assert_eq!(slot.read(), 42);
    }

    let addr = &LOCAL as *const AtomicU32 as usize;
    let local = unsafe { &*ptr::with_exposed_provenance::<AtomicU32>(addr) };
    assert_eq!(local.load(Ordering::Relaxed), 7);
}
