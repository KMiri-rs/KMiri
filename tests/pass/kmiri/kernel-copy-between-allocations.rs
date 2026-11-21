//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance

//! `kern_miri_copy` between two Rust objects behaves like `ptr::copy`: both
//! sides are interpreter memory, found through plain integer addresses (as a
//! kernel passing `usize` addresses does).
//!
//! Expected: `dst` equals `src`.

#[path = "../../../dev/test_utils/physical_copy.rs"]
mod utils;

use std::ptr;

use utils::*;

fn main() {
    let src: [u64; 4] = [1, 2, 3, 4];
    let mut dst = [0u64; 4];
    unsafe {
        kern_miri_copy(
            ptr::without_provenance_mut(dst.as_mut_ptr().expose_provenance()),
            ptr::without_provenance(src.as_ptr().expose_provenance()),
            size_of_val(&src),
        );
    }
    assert_eq!(dst, src);
}
