//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance

//! `kern_miri_copy` writing past the end of a large heap object. The object
//! (one page at DST, placed by `kern_miri_alloc_heap_slot`) ends on a page
//! boundary and an untyped page follows it; the copy is out of bounds anyway,
//! as a write through a pointer to the object would be.
//!
//!   pattern[0..100] --copy--> DST+4046..DST+4146 (object ends at DST+4096)
//!
//! Expected: Undefined Behavior (out-of-bounds access).

#[path = "../../../dev/test_utils/physical_copy.rs"]
mod utils;

use std::ptr;

use utils::*;

unsafe extern "Rust" {
    fn kern_miri_alloc_heap_slot(ptr: *mut u8, size: usize);
}

fn main() {
    unsafe {
        prepare_paging();
        kern_miri_alloc_pages(DST, 2);
        kern_miri_alloc_heap_slot(ptr::with_exposed_provenance_mut::<u8>(LINEAR + DST), 4096);

        let pattern = [1u8; 100];
        kern_miri_copy(at(LINEAR + DST + 4046), pattern.as_ptr().cast(), pattern.len()); //~ ERROR: Undefined Behavior
    }
}
