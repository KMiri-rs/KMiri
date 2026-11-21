//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance

//! Uninitialized bytes stay uninitialized through every kind of
//! `kern_miri_copy`, as through the native `volatile_copy_memory`:
//!
//!   buf (bytes 0..8 init, 8..16 uninit)
//!     --copy--> SRC page                (Rust object -> untyped)
//!     --copy--> DST+4090..DST+4106      (untyped -> untyped, crossing a page)
//!     --copy--> back                    (untyped -> Rust object)
//!
//! Expected: reading `back[8]` is Undefined Behavior (uninitialized memory).

#[path = "../../../dev/test_utils/physical_copy.rs"]
mod utils;

use std::mem::MaybeUninit;

use utils::*;

fn main() {
    unsafe {
        prepare_paging();
        kern_miri_alloc_pages(SRC, 1);
        kern_miri_alloc_pages(DST, 2);

        let mut buf = [MaybeUninit::<u8>::uninit(); 16];
        for (i, b) in buf[..8].iter_mut().enumerate() {
            b.write(i as u8 + 1);
        }
        kern_miri_copy(at(LINEAR + SRC), buf.as_ptr().cast(), buf.len());
        kern_miri_copy(at(LINEAR + DST + 4090), at(LINEAR + SRC), buf.len());

        let mut back = [MaybeUninit::<u8>::uninit(); 16];
        kern_miri_copy(back.as_mut_ptr().cast(), at(LINEAR + DST + 4090), back.len());
        assert_eq!(back[7].assume_init(), 8);
        let _val = *back.as_ptr().cast::<u8>().add(8); //~ ERROR: Undefined Behavior
    }
}
