//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2-irq.toml
//! A halted CPU wakes immediately when an IPI is queued to it.

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;
#[path = "../../../dev/test_utils/smp.rs"]
mod smp;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

static FLAG: AtomicBool = AtomicBool::new(false);
static CPU1_DONE: AtomicBool = AtomicBool::new(false);
static LOOPS: AtomicUsize = AtomicUsize::new(0);

extern "Rust" fn handler(_vector: u8) {}

fn cpu1_entry(_: usize) {
    let mut n = 0usize;
    while !FLAG.load(Ordering::Acquire) {
        irq::halt();
        n += 1;
    }
    LOOPS.store(n, Ordering::Release);
    CPU1_DONE.store(true, Ordering::Release);
}

fn main() {
    irq::register_entry(handler);
    smp::start(1, cpu1_entry, 0);
    // Let CPU 1 enter halt.
    for _ in 0..5 {
        std::hint::spin_loop();
    }
    FLAG.store(true, Ordering::Release);
    irq::send_ipi(1, 40);
    while !CPU1_DONE.load(Ordering::Acquire) {
        std::hint::spin_loop();
    }
    let n = LOOPS.load(Ordering::Acquire);
    assert!(n <= 3, "expected <= 3 halt loops, got {n}");
}
