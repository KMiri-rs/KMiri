//! A heap object that exactly fills its slab slot: the end pointer of its
//! iterator is the start address of the next slot. That address must not
//! turn into an allocation in the next slot, which a later object of a
//! different size then gets.
//!
//! Expected: no Undefined Behavior, and the kernel exits normally.
#![no_std]
#![deny(unsafe_code)]
#![cfg_attr(miri, feature(format_args_nl))]

extern crate alloc;

use alloc::{boxed::Box, vec, vec::Vec};

#[cfg_attr(miri, ostd::ktest::miri_main)]
#[cfg_attr(not(miri), ostd::main)]
fn kernel_main() {
    let mut sum = 0;
    for _ in 0..8 {
        // 32 bytes: exactly one 32-byte slot.
        let v = vec![1u64, 2, 3, 4];
        for x in v {
            sum += x;
        }
        // Objects of other sizes in the same size class take the next slots.
        let a: Box<[u8; 17]> = Box::new([7; 17]);
        let b: Box<[u8; 25]> = Box::new([9; 25]);
        sum += (a[16] + b[24]) as u64;
    }
    let big: Vec<[u64; 32]> = (0..8).map(|i| [i; 32]).collect(); // 2048 bytes
    for row in big {
        sum += row[31];
    }
    assert_eq!(sum, 8 * (10 + 16) + 28);
    #[cfg(miri)]
    ostd::miri_println!("heap-slot-exact-tail: sum {}", sum);
}
