#![no_std]
#![feature(format_args_nl)]

extern crate alloc;
use alloc::vec::Vec;

use ostd::mm::{FrameAllocOptions, HasPaddr};

// Regression test for https://github.com/KMiri-rs/KMiri/issues/267.
//
// Dropping a `Frame` gives its page back to the frame allocator, which can
// hand the same page out again. OSTD must tell KMiri that the page is free
// (`kern_miri_dealloc_pages`). Otherwise the next allocation of that page
// finds it `Untyped` instead of `Unused`, and KMiri reports page-state UB.
//
// Expected: no UB, and the kernel prints `frame-realloc: ok`.
#[ostd::ktest::miri_main]
fn kernel_main() {
    let options = FrameAllocOptions::new();
    let freed = options.alloc_frame().unwrap();
    let paddr = freed.paddr();
    drop(freed);

    // Keep allocating until the allocator returns the freed page.
    let mut frames = Vec::new();
    for _ in 0..16 {
        let frame = options.alloc_frame().unwrap();
        if frame.paddr() == paddr {
            ostd::miri_println!("frame-realloc: ok");
            return;
        }
        frames.push(frame);
    }
    panic!("the freed page was not allocated again");
}
