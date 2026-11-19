//! A slab whose slot size is not a multiple of the pointer alignment writes
//! misaligned free-list links (Asterinas #3911), even when the write stays
//! inside the page.
//!
//! `Slab::<2052>::new()` puts slots at page offsets 0 and 2052 and pushes each
//! onto the free list, which writes a `usize` link at the slot start. The link
//! of the slot at offset 2052 is an 8-byte write at an address that is 4 mod 8;
//! it ends at offset 2060, inside the page.
//!
//! Expected: Undefined Behavior (misaligned access) at the link write in
//! `SlabSlotList::push`, called from `Slab::new`.
//! Control: `tests/slab-slot-control`.
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
    note!("slab-misaligned-slot: creating Slab::<2052>");
    let slab = Slab::<2052>::new().unwrap();
    note!("slab-misaligned-slot: the misaligned link write was not reported");
    drop(slab);
}
