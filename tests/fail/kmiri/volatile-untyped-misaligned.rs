//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance

//! An aligned volatile load from an untyped page still requires the address
//! to be aligned for the type.
//!
//! Expected: `volatile_load::<u32>` at SRC+1 is UB.

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
        kern_miri_zero(SRC, 1);

        volatile_load::<u32>(at(LINEAR + SRC + 1).cast()); //~ ERROR: alignment
    }
}
