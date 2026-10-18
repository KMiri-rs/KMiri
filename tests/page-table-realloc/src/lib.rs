#![no_std]
#![feature(format_args_nl)]

extern crate alloc;
use alloc::vec::Vec;

use ostd::mm::{FrameAllocOptions, vm_space::VmSpace};

// Regression test for https://github.com/KMiri-rs/KMiri/issues/268.
//
// `VmSpace::new` allocates a root page-table frame, which KMiri marks as
// `Typed { page_type: PageTable, .. }`. Dropping the `VmSpace` frees that
// frame. OSTD must tell KMiri that the page is free
// (`kern_miri_dealloc_pages`). Otherwise allocating the page again as a plain
// frame finds it still `Typed`, and KMiri reports page-state UB.
//
// Expected: no UB, and the kernel prints `page-table-realloc: ok`.
#[ostd::ktest::miri_main]
fn kernel_main() {
    drop(VmSpace::new());

    // Allocate enough frames for the freed page-table page to come back.
    let options = FrameAllocOptions::new();
    let frames: Vec<_> = (0..16).map(|_| options.alloc_frame().unwrap()).collect();
    drop(frames);
    ostd::miri_println!("page-table-realloc: ok");
}
