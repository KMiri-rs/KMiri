//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance

//! `kern_miri_copy` from a freed object through a pointer that still carries
//! the object's provenance. The pointer names the object, so the copy is a
//! use-after-free, as the same copy written in Rust would be.
//!
//! Expected: Undefined Behavior (use-after-free).

#[path = "../../../dev/test_utils/physical_copy.rs"]
mod utils;

use utils::*;

fn main() {
    unsafe {
        prepare_paging();
        kern_miri_alloc_pages(SRC, 1);
        let obj = Box::new([7u8; 16]);
        let stale = obj.as_ptr();
        drop(obj);
        kern_miri_copy(at(LINEAR + SRC), stale, 16); //~ ERROR: Undefined Behavior
    }
}
