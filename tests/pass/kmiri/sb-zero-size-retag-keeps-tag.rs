//@compile-flags: -Zkmiri-toml=dev/test_utils/sb-zero-size-retag.toml

//! With `sb_zero_size_retag_keeps_tag = true`, a reference to a zero-sized
//! place keeps the tag of the pointer it was derived from, so a pointer
//! decoded from it can still access the memory behind it.
//!
//! This is what `bitvec` does: `&BitSlice` is a reference to a zero-sized
//! type whose address and length encode a pointer into the real storage.
//! `view` below is the same construction. Stacked Borrows gives a zero-size
//! retag a fresh tag that is in no borrow stack (see the fail test
//! `sb-zero-size-retag-fresh-tag.rs`); Tree Borrows accepts the program.
//!
//! Expected: the read through the decoded pointer succeeds.

fn main() {
    let data = [1u64, 2];
    let encoded = core::ptr::slice_from_raw_parts(data.as_ptr().cast::<()>(), data.len());
    // A zero-size retag: `[(); 2]` occupies no bytes.
    let view: &[()] = unsafe { &*encoded };
    let decoded = view.as_ptr().cast::<u64>();
    assert_eq!(unsafe { *decoded.add(1) }, 2);
}
