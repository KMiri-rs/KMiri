//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance

//! `kern_miri_copy` copies uninitialized bytes of a Rust object into untyped
//! pages and keeps them uninitialized, like the native `volatile_copy_memory`.
//!
//!   buf (bytes 0..8 init, 8..16 uninit) --copy--> SRC page --copy--> back
//!
//! Expected: no error; `back[0..8]` equals `buf[0..8]`.

#[path = "../../../dev/test_utils/physical_copy.rs"]
mod utils;

use std::mem::MaybeUninit;

use utils::*;

fn main() {
    unsafe {
        prepare_paging();
        kern_miri_alloc_pages(SRC, 1);

        let mut buf = [MaybeUninit::<u8>::uninit(); 16];
        for (i, b) in buf[..8].iter_mut().enumerate() {
            b.write(i as u8 + 1);
        }
        kern_miri_copy(at(LINEAR + SRC), buf.as_ptr().cast(), buf.len());

        let mut back = [MaybeUninit::<u8>::uninit(); 16];
        kern_miri_copy(back.as_mut_ptr().cast(), at(LINEAR + SRC), back.len());
        for i in 0..8 {
            assert_eq!(back[i].assume_init(), i as u8 + 1);
        }
    }
}
