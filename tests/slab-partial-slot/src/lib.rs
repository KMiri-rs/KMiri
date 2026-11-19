//! A slab whose slot size does not divide the page size must not write past
//! its page (Asterinas #3911).
//!
//! `Slab::<4095>::new()` puts slots at page offsets 0 and 4095 and pushes
//! each onto the free list, which writes a `usize` link at the slot start.
//! The link of the slot at offset 4095 is an 8-byte write at an odd address
//! that runs 7 bytes into the next page, which the slab does not own.
//!
//! Expected: Undefined Behavior at the link write in `SlabSlotList::push`,
//! called from `Slab::new`. Control: `tests/slab-slot-control`.
#![no_std]
#![deny(unsafe_code)]
#![cfg_attr(miri, feature(format_args_nl))]

extern crate alloc;

use ostd::mm::heap::Slab;

// Diagnostics only; compiled out outside Miri.
macro_rules! note {
    ($($t:tt)*) => { #[cfg(miri)] ostd::miri_println!($($t)*); };
}

#[cfg_attr(miri, ostd::ktest::miri_main)]
#[cfg_attr(not(miri), ostd::main)]
fn kernel_main() {
    note!("slab-partial-slot: creating Slab::<4095>");
    let slab = Slab::<4095>::new().unwrap();
    note!("slab-partial-slot: the out-of-page link write was not reported");
    drop(slab);
}
