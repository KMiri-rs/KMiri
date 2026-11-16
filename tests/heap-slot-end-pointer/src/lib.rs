//! The one-past-the-end pointer of a heap object must not create an
//! allocation in the next slab slot.
//!
//! `buffer` exactly fills a 64-byte slot, so its one-past-the-end address is
//! the address of the next slot, which is free. Dropping the exhausted
//! `IntoIter` builds an empty slice at that address. KMiri used to create an
//! allocation for any address in a slab page on first use, so this made the
//! free slot look occupied, and the allocator handing it out later failed
//! with "heap slot ... already has a live 64-byte allocation". The full
//! Asterinas kernel hits this while it initializes its LSM modules.
//!
//! Expected: all allocations below succeed and the kernel exits normally.
#![no_std]
#![deny(unsafe_code)]
#![feature(format_args_nl)]

extern crate alloc;

use alloc::{boxed::Box, vec::Vec};

#[ostd::ktest::miri_main]
fn kernel_main() {
    let mut buffer: Vec<[u64; 4]> = Vec::with_capacity(2);
    buffer.push([1; 4]);
    buffer.push([2; 4]);
    assert_eq!(buffer.capacity() * size_of::<[u64; 4]>(), 64);

    let mut iter = buffer.into_iter();
    assert_eq!(iter.by_ref().map(|item| item[0]).sum::<u64>(), 3);
    drop(iter);
    ostd::miri_println!("heap-slot-end-pointer: exhausted IntoIter dropped");

    // One of these lands in the slot after `buffer`.
    let boxes: Vec<Box<[u64; 5]>> = (0..8).map(|i| Box::new([i; 5])).collect();
    assert_eq!(boxes.iter().map(|b| b[0]).sum::<u64>(), 28);
    ostd::miri_println!("heap-slot-end-pointer: next slots allocated");
}
