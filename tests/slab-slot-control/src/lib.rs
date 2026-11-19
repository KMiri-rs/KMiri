//! Control for `tests/slab-partial-slot`: a slab whose slot size divides the
//! page size keeps every free-list link inside its page.
//!
//! Creates a `Slab::<64>`, takes every slot out of it and puts them all back,
//! so that each free-list link is written by `Slab::new`, read by `alloc` and
//! written again by `dealloc`, then drops the slab.
//!
//! Expected: no Undefined Behavior, and the kernel exits normally.
#![no_std]
#![deny(unsafe_code)]
#![cfg_attr(miri, feature(format_args_nl))]

extern crate alloc;

use alloc::vec::Vec;

use ostd::mm::heap::Slab;

// Diagnostics only; compiled out outside Miri.
macro_rules! note {
    ($($t:tt)*) => { #[cfg(miri)] ostd::miri_println!($($t)*); };
}

#[cfg_attr(miri, ostd::ktest::miri_main)]
#[cfg_attr(not(miri), ostd::main)]
fn kernel_main() {
    let mut slab = Slab::<64>::new().unwrap();
    let capacity = slab.meta().capacity();
    let mut slots = Vec::new();
    for _ in 0..capacity {
        slots.push(slab.meta_mut().alloc().unwrap());
    }
    assert_eq!(slab.meta().nr_allocated(), capacity);
    for slot in slots {
        slab.dealloc(slot).unwrap();
    }
    assert_eq!(slab.meta().nr_allocated(), 0);
    // The free list is intact: the slots come out again.
    let slot = slab.meta_mut().alloc().unwrap();
    slab.dealloc(slot).unwrap();
    drop(slab);
    note!("slab-slot-control: {} slots taken and returned", capacity);
}
