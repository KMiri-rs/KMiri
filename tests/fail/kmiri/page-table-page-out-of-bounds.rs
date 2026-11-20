//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance

//! The page-table page object ends at the page end: pointer arithmetic from the
//! page start past one-past-the-end of the 4096-byte page is out of bounds.
//!
//!   NODE (retyped PageTable)
//!   ┌──────┬─────┬────────┐
//!   │  e0  │ ... │  e511  │ e512 (one past end, allowed)   e513 ✗
//!   └──────┴─────┴────────┘

use std::ptr;

#[path = "../../../dev/test_utils/physical_copy.rs"]
mod utils;

use utils::*;

const NODE: usize = 0x0402_0000;

fn main() {
    unsafe {
        prepare_paging();
        kern_miri_alloc_pages(NODE, 2);
        kern_miri_retype_pages(NODE, 2, TypedKind::PageTable, 8);
        let base = ptr::with_exposed_provenance_mut::<u64>(KERNEL + NODE);
        base.write(1);
        let _ = base.add(513); //~ ERROR: in-bounds pointer arithmetic failed
    }
}
