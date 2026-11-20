//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance

//! Freeing a page-table page frees the page object: an entry read through the
//! old address afterwards is a use of freed memory.

use std::ptr;

#[path = "../../../dev/test_utils/physical_copy.rs"]
mod utils;

use utils::*;

unsafe extern "Rust" {
    fn kern_miri_dealloc_pages(paddr: usize, count: usize);
}

const NODE: usize = 0x0402_0000;

fn main() {
    unsafe {
        prepare_paging();
        kern_miri_alloc_pages(NODE, 1);
        kern_miri_retype_pages(NODE, 1, TypedKind::PageTable, 8);
        let entry = ptr::with_exposed_provenance_mut::<u64>(KERNEL + NODE + 5 * 8);
        entry.write(0x1003);
        kern_miri_dealloc_pages(NODE, 1);
        let _ = entry.read(); //~ ERROR: Undefined Behavior
    }
}
