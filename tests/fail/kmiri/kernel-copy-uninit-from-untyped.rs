//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance

//! `kern_miri_copy` from an untyped page into a Rust object carries the
//! initialization state of every byte along.
//!
//!   SRC page: [ 7 | uninit ]  --copy 2 bytes-->  buf: [ 7 | uninit ]
//!
//! Only the first byte of the freshly allocated page was written, so only
//! `buf[0]` may be read afterwards.
//!
//! Expected: reading `buf[0]` is fine, reading `buf[1]` is UB.

use std::hint::black_box;
use std::mem::MaybeUninit;

#[path = "../../../dev/test_utils/physical_copy.rs"]
mod utils;

use utils::*;

fn main() {
    unsafe {
        prepare_paging();
        kern_miri_alloc_pages(SRC, 1);
        kern_miri_write_bytes(SRC, [7u8].as_ptr(), 1);

        let mut buf = [MaybeUninit::<u8>::new(0); 2];
        kern_miri_copy(buf.as_mut_ptr().cast(), at(LINEAR + SRC), 2);
        assert_eq!(black_box(buf[0].assume_init()), 7);
        black_box(buf[1].assume_init()); //~ ERROR: uninitialized
    }
}
