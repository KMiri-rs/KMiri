//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance

//! `kern_miri_copy` between untyped pages and a large heap object. The kernel
//! places such an object directly on frames (`kern_miri_alloc_heap_slot` on
//! untyped pages), so its pages stay untyped, but the object is an interpreter
//! allocation: the copy must go through it, so that Rust accesses to the object
//! see the result.
//!
//!   SRC[0..100] --copy--> obj[4046..4146] (crosses the object's page boundary)
//!                                          then read obj in Rust
//!   obj[10..20] written in Rust --copy--> SRC+200 --copy--> out
//!
//! Expected: the object holds SRC's bytes, and `out` holds the bytes written.

#[path = "../../../dev/test_utils/physical_copy.rs"]
mod utils;

use std::ptr;

use utils::*;

unsafe extern "Rust" {
    fn kern_miri_alloc_heap_slot(ptr: *mut u8, size: usize);
}

fn main() {
    unsafe {
        prepare_paging();
        kern_miri_alloc_pages(SRC, 1);
        kern_miri_alloc_pages(DST, 2);
        let obj = ptr::with_exposed_provenance_mut::<u8>(LINEAR + DST);
        kern_miri_alloc_heap_slot(obj, 8192);

        let pattern: [u8; 100] = core::array::from_fn(|i| i as u8 + 1);
        kern_miri_write_bytes(SRC, pattern.as_ptr(), pattern.len());
        kern_miri_copy(at(LINEAR + DST + 4046), at(LINEAR + SRC), pattern.len());
        for i in 0..100 {
            assert_eq!(obj.add(4046 + i).read(), i as u8 + 1);
        }

        for i in 0..10 {
            obj.add(10 + i).write(0xa0 + i as u8);
        }
        kern_miri_copy(at(LINEAR + SRC + 200), at(LINEAR + DST + 10), 10);
        let mut out = [0u8; 10];
        kern_miri_copy(out.as_mut_ptr().cast(), at(LINEAR + SRC + 200), out.len());
        assert_eq!(out, core::array::from_fn(|i| 0xa0 + i as u8));
    }
}
