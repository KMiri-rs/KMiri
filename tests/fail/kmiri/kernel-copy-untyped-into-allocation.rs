//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance

//! `kern_miri_copy` whose source starts on an untyped page and runs into a
//! large heap object placed on the next page. The object's bytes belong to
//! the object, not to the page, and a copy spanning both kinds of memory is
//! not supported.
//!
//!   DST page 0 (untyped) | DST page 1 (heap object)
//!          [4000 ........|.... 4200) --copy--> buf
//!
//! Expected: unsupported operation.

#[path = "../../../dev/test_utils/physical_copy.rs"]
mod utils;

use utils::*;

unsafe extern "Rust" {
    fn kern_miri_alloc_heap_slot(ptr: *mut u8, size: usize);
}

fn main() {
    unsafe {
        prepare_paging();
        kern_miri_alloc_pages(DST, 2);
        kern_miri_alloc_heap_slot(at(LINEAR + DST + 4096), 4096);

        let mut buf = [0u8; 200];
        kern_miri_copy(buf.as_mut_ptr(), at(LINEAR + DST + 4000), buf.len()); //~ ERROR: unsupported operation
    }
}
