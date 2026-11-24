//! A long loop inside one stack frame must not run out of stack addresses.
//!
//! `slot` becomes live and dead again in every iteration, and taking its
//! address forces it into memory, so every iteration needs 4 KiB of stack
//! addresses. KMiri hands out stack addresses downwards from a per-thread
//! stack pointer. If that pointer only moves back up when a frame is popped,
//! 10000 iterations need 40 MiB and run past the bottom of the 12 MiB main
//! stack ("no more free addresses in the address space"), although only one
//! `slot` is live at any time.
//!
//! Expected: the loop finishes, and `slot` had the same address every time.
use std::hint::black_box;

fn main() {
    let (mut lowest, mut highest) = (usize::MAX, 0);
    for i in 0..10_000usize {
        let slot = [i; 512];
        let addr = black_box(&slot).as_ptr() as usize;
        lowest = lowest.min(addr);
        highest = highest.max(addr);
    }
    assert_eq!(lowest, highest);
}
