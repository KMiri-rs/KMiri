//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2.toml
//! The program ends when the bootstrap context returns, as a kernel powers the
//! machine off: CPU 1 is still spinning then, and that is not a leaked thread.

#[path = "../../../dev/test_utils/smp.rs"]
mod smp;

use std::hint::spin_loop;
use std::sync::atomic::{AtomicBool, Ordering};

static STARTED: AtomicBool = AtomicBool::new(false);

fn cpu1(_: usize) {
    STARTED.store(true, Ordering::Release);
    loop {
        spin_loop();
    }
}

fn main() {
    smp::start(1, cpu1, 0);
    while !STARTED.load(Ordering::Acquire) {
        spin_loop();
    }
}
