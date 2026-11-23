//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance

//! A volatile load from an untyped page carries the initialization state of
//! the bytes along, like any other load.
//!
//!   SRC page: [ 7 | uninit ]
//!
//! Expected: loading the byte at SRC is fine, loading the one at SRC+1 is UB.

#![feature(core_intrinsics)]
#![allow(internal_features)]

use core::intrinsics::volatile_load;

#[path = "../../../dev/test_utils/physical_copy.rs"]
mod utils;

use utils::*;

fn main() {
    unsafe {
        prepare_paging();
        kern_miri_alloc_pages(SRC, 1);
        kern_miri_write_bytes(SRC, [7u8].as_ptr(), 1);

        assert_eq!(volatile_load(at(LINEAR + SRC)), 7u8);
        volatile_load::<u8>(at(LINEAR + SRC + 1)); //~ ERROR: uninitialized
    }
}
