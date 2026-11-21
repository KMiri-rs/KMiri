//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance

//! `kern_miri_copy` copies between interpreter memory (a Rust object) and
//! untyped pages, in both directions.
//!
//!   pattern (Rust array) --copy--> SRC page (untyped) --copy--> back (Rust array)
//!
//! This is what a kernel does when it writes one of its buffers into a frame
//! (e.g. a page cache page) and reads it back. The page is never typed, so the
//! second copy is the only way to see its contents.
//!
//! Expected: `back` equals `pattern`.

#[path = "../../../dev/test_utils/physical_copy.rs"]
mod utils;

use utils::*;

fn main() {
    unsafe {
        prepare_paging();
        kern_miri_alloc_pages(SRC, 1);

        let pattern: [u8; 17] = core::array::from_fn(|i| i as u8 + 1);
        kern_miri_copy(at(LINEAR + SRC + 5), pattern.as_ptr().cast(), pattern.len());

        let mut back = [0u8; 17];
        kern_miri_copy(back.as_mut_ptr().cast(), at(LINEAR + SRC + 5), back.len());
        assert_eq!(back, pattern);
    }
}
