//! By default, Stacked Borrows gives a zero-size retag a fresh tag that is in
//! no borrow stack, so a pointer decoded from a reference to a zero-sized
//! place cannot access memory. The KMiri option
//! `sb_zero_size_retag_keeps_tag` changes this; see the pass test
//! `sb-zero-size-retag-keeps-tag.rs`, which is the same program.
//!
//! Expected: without the option, the read through the decoded pointer is UB.

fn main() {
    let data = [1u64, 2];
    let encoded = core::ptr::slice_from_raw_parts(data.as_ptr().cast::<()>(), data.len());
    // A zero-size retag: `[(); 2]` occupies no bytes.
    let view: &[()] = unsafe { &*encoded };
    let decoded = view.as_ptr().cast::<u64>();
    assert_eq!(unsafe { *decoded.add(1) }, 2); //~ ERROR: does not exist in the borrow stack
}
