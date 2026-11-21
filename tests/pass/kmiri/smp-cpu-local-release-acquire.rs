//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2.toml
//! Control for `smp-cpu-local-race`: the same two writes to a `.cpu_local`
//! variable, ordered by a Release store and an Acquire load, are not a race.

#[path = "../../../dev/test_utils/smp.rs"]
mod smp;

use std::cell::UnsafeCell;
use std::hint::spin_loop;
use std::sync::atomic::{AtomicBool, Ordering};

struct Plain(UnsafeCell<usize>);
unsafe impl Sync for Plain {}

#[unsafe(link_section = ".cpu_local")]
static X: Plain = Plain(UnsafeCell::new(0));
static DONE: AtomicBool = AtomicBool::new(false);

fn entry(_: usize) {
    unsafe { *X.0.get() = 1 };
    DONE.store(true, Ordering::Release);
}

fn main() {
    smp::start(1, entry, 0);
    while !DONE.load(Ordering::Acquire) {
        spin_loop();
    }
    unsafe { *X.0.get() = 2 };
    assert_eq!(unsafe { *X.0.get() }, 2);
}
