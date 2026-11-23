//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance

//! A volatile fill that starts on an untyped page but runs past the end of
//! the allocated pages is not let through.
//!
//!   SRC page (untyped) | SRC+4096 (never allocated)
//!   volatile_set_memory SRC+4000, 200 bytes
//!
//! Expected: the access is reported when it reaches SRC+4096.

#![feature(core_intrinsics)]
#![allow(internal_features)]

use core::intrinsics::volatile_set_memory;

#[path = "../../../dev/test_utils/physical_copy.rs"]
mod utils;

use utils::*;

fn main() {
    unsafe {
        prepare_paging();
        kern_miri_alloc_pages(SRC, 1);

        volatile_set_memory(at(LINEAR + SRC + 4000), 0, 200); //~ ERROR: 0xffff800004001000
    }
}
