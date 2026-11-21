//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2.toml
//! A release store and an acquire load order a plain write on one CPU before
//! a plain write on another: no data race.

#[path = "../../../dev/test_utils/smp.rs"]
mod smp;

use std::cell::UnsafeCell;
use std::hint::spin_loop;
use std::sync::atomic::{AtomicBool, Ordering};

struct Plain(UnsafeCell<usize>);
unsafe impl Sync for Plain {}

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
}
