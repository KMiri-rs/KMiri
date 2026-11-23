//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance

//! Volatile accesses through plain addresses on untyped pages, as the kernel's
//! I/O helpers make them (`fill`, `read_val`/`write_val`, `read_once`/`write_once`).
//!
//!   SRC+4000..SRC+4300  volatile_set_memory 0xab (crosses a page boundary)
//!   SRC+8               write_volatile / read_volatile u64
//!   SRC+4094            unaligned_volatile_store / load u32 (crosses a page boundary)
//!   DST                 volatile_set_memory::<u32> 0x5a, count 4
//!
//! Expected: every value reads back as written, and volatile accesses to a
//! Rust object still go through the object as before.

#![feature(core_intrinsics)]
#![allow(internal_features)]

use core::intrinsics::{unaligned_volatile_load, unaligned_volatile_store, volatile_set_memory};

#[path = "../../../dev/test_utils/physical_copy.rs"]
mod utils;

use utils::*;

fn main() {
    unsafe {
        prepare_paging();
        kern_miri_alloc_pages(SRC, 2);
        kern_miri_alloc_pages(DST, 1);

        volatile_set_memory(at(LINEAR + SRC + 4000), 0xab, 300);
        let mut back = [0u8; 300];
        kern_miri_copy(back.as_mut_ptr(), at(LINEAR + SRC + 4000), back.len());
        assert_eq!(back, [0xab; 300]);

        let p = at(LINEAR + SRC + 8).cast::<u64>();
        p.write_volatile(0x1122_3344_5566_7788);
        assert_eq!(p.read_volatile(), 0x1122_3344_5566_7788);

        let q = at(LINEAR + SRC + 4094).cast::<u32>();
        unaligned_volatile_store(q, 0xdead_beef);
        assert_eq!(unaligned_volatile_load(q), 0xdead_beef);

        volatile_set_memory(at(LINEAR + DST).cast::<u32>(), 0x5a, 4);
        let mut words = [0u32; 4];
        kern_miri_copy(words.as_mut_ptr().cast(), at(LINEAR + DST), 16);
        assert_eq!(words, [0x5a5a_5a5a; 4]);

        let mut local = 0u32;
        (&raw mut local).write_volatile(7);
        assert_eq!((&raw const local).read_volatile(), 7);
    }
}
