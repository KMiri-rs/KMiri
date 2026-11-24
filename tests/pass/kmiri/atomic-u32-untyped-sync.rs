//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2-paging.toml -Zmiri-permissive-provenance
//! An atomic compare-exchange on an untyped word synchronizes like a
//! sequentially consistent one: CPU 1 writes `X` and then publishes with a
//! compare-exchange of the word; CPU 0 waits for it with an atomic load and then
//! writes `X`. The two writes are ordered, so there is no data race.

#[path = "../../../dev/test_utils/smp.rs"]
mod smp;
#[path = "../../../dev/test_utils/physical_copy.rs"]
mod utils;

use std::cell::UnsafeCell;
use std::hint::spin_loop;

use utils::*;

extern "Rust" {
    fn kern_miri_atomic_load_u32(ptr: *const u32) -> u32;
    fn kern_miri_atomic_cmpxchg_u32(ptr: *mut u32, old_val: u32, new_val: u32) -> u32;
}

struct Plain(UnsafeCell<usize>);
unsafe impl Sync for Plain {}

static X: Plain = Plain(UnsafeCell::new(0));
/// CPU 1 runs on a stack in the mapped window.
const CPU1_STACK_END: usize = LINEAR + 0x0620_0000;

fn flag() -> *mut u32 {
    at(LINEAR + SRC + 8).cast()
}

fn entry(_: usize) {
    unsafe { *X.0.get() = 1 };
    assert_eq!(unsafe { kern_miri_atomic_cmpxchg_u32(flag(), 0, 1) }, 0);
}

fn main() {
    unsafe {
        prepare_paging();
        kern_miri_alloc_pages(SRC, 1);
        kern_miri_zero(SRC, 1);
    }
    smp::start_with_stack(1, entry, 0, CPU1_STACK_END);
    while unsafe { kern_miri_atomic_load_u32(flag()) } == 0 {
        spin_loop();
    }
    unsafe { *X.0.get() = 2 };
}
