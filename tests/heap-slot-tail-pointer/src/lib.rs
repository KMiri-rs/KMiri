//! A pointer one past the end of a heap object that is smaller than its slab
//! slot is in no allocation; KMiri must not create a second allocation in the
//! slot for it.
//!
//! Consumes `Vec`s by value. Dropping the `IntoIter` builds an empty slice at
//! the iterator's position, which by then is one past the end of the buffer:
//! 24 bytes in a 32-byte slot and 1536 bytes in a 2048-byte slot.
//!
//! Expected: the kernel exits normally.
#![no_std]
#![deny(unsafe_code)]
#![cfg_attr(miri, feature(format_args_nl))]

extern crate alloc;

use alloc::{vec, vec::Vec};

#[cfg_attr(miri, ostd::ktest::miri_main)]
#[cfg_attr(not(miri), ostd::main)]
fn kernel_main() {
    let small: Vec<u64> = vec![1, 2, 3];
    let mut sum = 0;
    for x in small {
        sum += x;
    }
    assert_eq!(sum, 6);

    let large: Vec<[u64; 3]> = (0..64).map(|i| [i, i, i]).collect();
    assert_eq!(large.capacity(), 64);
    let mut sum = 0;
    for x in large {
        sum += x[0];
    }
    assert_eq!(sum, 63 * 64 / 2);
    #[cfg(miri)]
    ostd::miri_println!("heap-slot-tail-pointer: both vectors consumed");
}
