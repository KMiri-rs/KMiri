//! A heap object that does not fit in a slab slot must be usable under KMiri.
//!
//! OSTD serves such an allocation from a "large slot": frames taken directly
//! from the frame allocator, addressed through `paddr_to_vaddr`, i.e. through
//! an address computed from an integer. KMiri has to create the object's
//! allocation when OSTD reports the new heap object; otherwise the first
//! access fails with "dangling pointer (it has no provenance)". The full
//! Asterinas kernel hits this during component initialization (the block
//! device ID bitmap is a 128 KiB `Vec<u64>`).
//!
//! Expected: every step below completes and the kernel exits normally.
#![no_std]
#![deny(unsafe_code)]
#![feature(format_args_nl)]

extern crate alloc;

use alloc::vec::Vec;

/// 16384 `u64`s = 128 KiB, the size of the block device ID bitmap.
const WORDS: usize = 128 * 1024 / 8;

#[ostd::ktest::miri_main]
fn kernel_main() {
    // A fresh large object starts uninitialized; `resize` writes all of it.
    let mut words: Vec<u64> = Vec::with_capacity(WORDS);
    words.resize(WORDS, 0);
    words[0] = 1;
    words[WORDS - 1] = 2;
    assert_eq!(words[0] + words[WORDS - 1], 3);
    ostd::miri_println!("heap-large-object: write and read ok");

    // Growing moves the contents to a second large object and frees the first.
    words.reserve_exact(WORDS);
    assert_eq!(words.capacity(), 2 * WORDS);
    assert_eq!(words[0] + words[WORDS - 1], 3);
    ostd::miri_println!("heap-large-object: grow ok");

    // After the drop, the frames can back a new large object.
    drop(words);
    let mut again: Vec<u64> = Vec::with_capacity(WORDS);
    again.push(4);
    assert_eq!(again[0], 4);
    ostd::miri_println!("heap-large-object: drop and reuse ok");
}
