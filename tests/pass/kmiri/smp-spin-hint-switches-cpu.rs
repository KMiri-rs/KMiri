//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2-no-switch.toml
//! With random CPU switches off, a spin-loop hint is what lets the CPU that
//! the spinning CPU waits for make progress.

#[path = "../../../dev/test_utils/smp.rs"]
mod smp;

use std::hint::spin_loop;
use std::sync::atomic::{AtomicBool, Ordering};

static READY: AtomicBool = AtomicBool::new(false);

fn entry(_: usize) {
    READY.store(true, Ordering::Release);
}

fn main() {
    smp::start(1, entry, 0);
    while !READY.load(Ordering::Acquire) {
        spin_loop();
    }
}
