//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance

//! `kern_miri_copy` reading past the end of a Rust array into an untyped page.
//! The array is accessed with the usual checks, so the copy is an out-of-bounds
//! access, as the same copy written in Rust would be.
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
        kern_miri_copy(at(LINEAR + SRC), buf.as_ptr().cast(), 20); //~ ERROR: Undefined Behavior
    }
}
