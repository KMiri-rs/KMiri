//! Reading a large heap object after it was freed must be reported as UB.
//!
//! Control for `tests/heap-large-object`: KMiri gives a large-slot object an
//! allocation so that valid accesses work; it must still end that allocation
//! when OSTD frees the object, so a stale read is a use-after-free.
//!
//! Expected: Undefined Behavior at the read below.
//! OSTD's allocator hands out a pointer made from an integer, so Miri finds
//! the object by address and reports a dangling pointer "(it has no
//! provenance)" rather than naming the free or the bound. Without the fix
//! the same message appears earlier, inside `Vec::resize`.
#![no_std]
#![feature(format_args_nl)]

extern crate alloc;

use alloc::vec::Vec;

/// 16384 `u64`s = 128 KiB, more than any slab slot.
const WORDS: usize = 128 * 1024 / 8;

#[ostd::ktest::miri_main]
fn kernel_main() {
    let mut words: Vec<u64> = Vec::with_capacity(WORDS);
    words.resize(WORDS, 7);
    let stale = words.as_ptr();
    drop(words);
    ostd::miri_println!("heap-large-object-uaf: reading the freed large object");
    // SAFETY: none; this read is the use-after-free under test.
    let _value = unsafe { stale.add(WORDS - 1).read() };
    ostd::miri_println!("heap-large-object-uaf: stale read was not reported");
}
