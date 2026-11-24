//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance

//! An atomic load of a word on an untyped page that was never written reads
//! uninitialized memory into a `u32`, as a volatile load there does.

#[path = "../../../dev/test_utils/physical_copy.rs"]
mod utils;

use utils::*;

extern "Rust" {
    fn kern_miri_atomic_load_u32(ptr: *const u32) -> u32;
}

fn main() {
    unsafe {
        prepare_paging();
        kern_miri_alloc_pages(SRC, 1);
        kern_miri_atomic_load_u32(at(LINEAR + SRC + 8).cast()); //~ ERROR: uninitialized
    }
}
