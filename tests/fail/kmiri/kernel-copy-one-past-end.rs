//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance

//! `kern_miri_copy` from one past the end of a Rust array, through a pointer
//! derived from the array. The pointer names the array, so the copy is out of
//! bounds even though it starts exactly where the array ends.
//!
//! Expected: Undefined Behavior (out-of-bounds access).

#[path = "../../../dev/test_utils/physical_copy.rs"]
mod utils;

use utils::*;

fn main() {
    unsafe {
        prepare_paging();
        kern_miri_alloc_pages(SRC, 1);
        let buf = [7u8; 16];
        kern_miri_copy(at(LINEAR + SRC), buf.as_ptr().add(16), 4); //~ ERROR: Undefined Behavior
    }
}
