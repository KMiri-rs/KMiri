//! Reading past the end of a live large heap object must be reported as UB.
//!
//! Control for `tests/heap-large-object`: the allocation KMiri creates for a
//! large-slot object must have the object's size, not the size of the frames
//! behind it, so a read one element past the end is out of bounds.
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
    // A size that is not a multiple of the page size, so the backing frames
    // extend past the object's end.
    let len = WORDS + 3;
    let mut words: Vec<u64> = Vec::with_capacity(len);
    words.resize(len, 7);
    assert_eq!(words.capacity(), len);
    ostd::miri_println!("heap-large-object-oob: reading one element past the end");
    // SAFETY: none; this read is the out-of-bounds access under test.
    let _value = unsafe { words.as_ptr().add(len).read() };
    ostd::miri_println!("heap-large-object-oob: out-of-bounds read was not reported");
}
