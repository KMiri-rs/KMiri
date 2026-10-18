#![no_std]
#![feature(format_args_nl)]

extern crate alloc;
use ostd::mm::FrameAllocOptions;

// Regression test for the Asterinas side of
// https://github.com/KMiri-rs/KMiri/issues/261.
//
// `FrameAllocOptions` zeroes new frames by default, which under Miri calls
// `kern_miri_zero`. For a segment, OSTD must first mark the pages allocated
// (`kern_miri_alloc_pages`), as it already does for a single frame. Otherwise
// KMiri zeroes pages that it never saw allocated.
//
// Expected: no error, and the kernel prints `segment-zeroed: ok`.
#[ostd::ktest::miri_main]
fn kernel_main() {
    let segment = FrameAllocOptions::new().alloc_segment(2).unwrap();
    drop(segment);
    ostd::miri_println!("segment-zeroed: ok");
}
