//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance

//! An aligned volatile store to an untyped page still requires the address
//! to be aligned for the type.
//!
//! Expected: `volatile_store::<u32>` at SRC+2 is UB.

#![feature(core_intrinsics)]
#![allow(internal_features)]

use core::intrinsics::volatile_store;

#[path = "../../../dev/test_utils/physical_copy.rs"]
mod utils;

use utils::*;

fn main() {
    unsafe {
        prepare_paging();
        kern_miri_alloc_pages(SRC, 1);

        volatile_store::<u32>(at(LINEAR + SRC + 2).cast(), 1); //~ ERROR: alignment
    }
}
