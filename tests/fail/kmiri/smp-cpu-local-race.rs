//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2.toml
//! A `.cpu_local` variable is ordinary memory for race detection. Here both
//! CPUs write the same copy of it (the one in the section) with nothing
//! ordering the two writes, which is a data race. CPU-local storage used to be
//! exempt from race detection, which hid races like this one.

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
    DONE.store(true, Ordering::Relaxed);
}

fn main() {
    smp::start(1, entry, 0);
    while !DONE.load(Ordering::Relaxed) {
        spin_loop();
    }
    unsafe { *X.0.get() = 2 }; //~ ERROR: Data race detected
}
