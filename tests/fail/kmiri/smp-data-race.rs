//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2.toml
//! Same as `smp-message-passing`, but the flag is relaxed: nothing orders the
//! two plain writes on different CPUs, which is a data race.

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
    DONE.store(true, Ordering::Relaxed);
}

fn main() {
    smp::start(1, entry, 0);
    while !DONE.load(Ordering::Relaxed) {
        spin_loop();
    }
    unsafe { *X.0.get() = 2 }; //~ ERROR: Data race detected
}
