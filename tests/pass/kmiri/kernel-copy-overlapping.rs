//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance

//! `kern_miri_copy` with overlapping ranges behaves like `memmove`, as the
//! native `volatile_copy_memory` does.
//!
//! 1. Within one Rust array: the interpreter's own copy.
//! 2. Within untyped pages, forwards over several page-bounded chunks:
//!    SRC+0..SRC+5000 --copy--> SRC+10..SRC+5010.
//!
//! Expected: both results are the source shifted, not smeared.

#[path = "../../../dev/test_utils/physical_copy.rs"]
mod utils;

use utils::*;

fn main() {
    unsafe {
        prepare_paging();

        let mut buf: [u8; 12] = core::array::from_fn(|i| i as u8);
        let base = buf.as_mut_ptr();
        kern_miri_copy(base.add(2), base, 10);
        assert_eq!(buf, [0, 1, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);

        kern_miri_alloc_pages(SRC, 2);
        let data: Vec<u8> = (0..5000).map(|i| (i % 251) as u8).collect();
        kern_miri_write_bytes(SRC, data.as_ptr(), data.len());
        kern_miri_copy(at(LINEAR + SRC + 10), at(LINEAR + SRC), data.len());
        let mut back = vec![0u8; 5010];
        kern_miri_copy(back.as_mut_ptr().cast(), at(LINEAR + SRC), back.len());
        assert_eq!(back[..10], data[..10]);
        assert_eq!(back[10..], data[..]);
    }
}
