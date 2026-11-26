//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2-irq.toml
//! An IPI sent while the target CPU has IRQs disabled is not delivered until
//! the target CPU re-enables IRQs.

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;
#[path = "../../../dev/test_utils/smp.rs"]
mod smp;

use std::hint::spin_loop;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const VECTOR: u8 = 41;
static CPU1_DISABLED: AtomicBool = AtomicBool::new(false);
static SENT: AtomicBool = AtomicBool::new(false);
static HANDLED_COUNT: AtomicUsize = AtomicUsize::new(0);
static CPU1_DONE: AtomicBool = AtomicBool::new(false);

extern "Rust" fn handler(vector: u8) {
    assert_eq!(vector, VECTOR);
    assert_eq!(smp::current(), 1);
    HANDLED_COUNT.fetch_add(1, Ordering::Release);
}

fn cpu1_entry(_: usize) {
    irq::disable();
    CPU1_DISABLED.store(true, Ordering::Release);
    while !SENT.load(Ordering::Acquire) {
        spin_loop();
    }
    for _ in 0..16 {
        spin_loop();
    }
    assert_eq!(HANDLED_COUNT.load(Ordering::Acquire), 0);
    irq::enable();
    while HANDLED_COUNT.load(Ordering::Acquire) == 0 {
        spin_loop();
    }
    assert_eq!(HANDLED_COUNT.load(Ordering::Acquire), 1);
    CPU1_DONE.store(true, Ordering::Release);
}

fn main() {
    irq::register_entry(handler);
    smp::start(1, cpu1_entry, 0);
    while !CPU1_DISABLED.load(Ordering::Acquire) {
        spin_loop();
    }
    irq::send_ipi(1, VECTOR);
    SENT.store(true, Ordering::Release);
    while !CPU1_DONE.load(Ordering::Acquire) {
        spin_loop();
    }
}
