//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance

//! A page-table page is one object: an array of 512 eight-byte entries. Code
//! reaches entry `i` by pointer arithmetic from the page start, as OSTD does
//! with `(paddr_to_vaddr(pt) as *mut Entry).add(i)`. This is in bounds for every
//! `i` in the page.
//!
//!   NODE (retyped PageTable, entry size 8)
//!   ┌──────┬──────┬─────┬────────┬─────┬────────┐
//!   │  e0  │  e1  │ ... │  e255  │ ... │  e511  │
//!   └──────┴──────┴─────┴────────┴─────┴────────┘
//!   base ──.add(i)──▶ e_i      (one allocation of 4096 bytes)

use std::ptr;

#[path = "../../../dev/test_utils/physical_copy.rs"]
mod utils;

use utils::*;

const NODE: usize = 0x0402_0000;

fn main() {
    unsafe {
        prepare_paging();
        kern_miri_alloc_pages(NODE, 1);
        kern_miri_retype_pages(NODE, 1, TypedKind::PageTable, 8);
        let base = ptr::with_exposed_provenance_mut::<u64>(KERNEL + NODE);
        for i in [0usize, 1, 2, 255, 511] {
            base.add(i).write(0x1000 * i as u64 | 1);
        }
        for i in [0usize, 1, 2, 255, 511] {
            assert_eq!(base.add(i).read(), 0x1000 * i as u64 | 1);
        }
    }
}
