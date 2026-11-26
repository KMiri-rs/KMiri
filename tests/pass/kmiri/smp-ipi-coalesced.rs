//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2-irq.toml
//! Two IPIs of the same vector sent while the target CPU has IRQs disabled
//! coalesce into one delivery; a subsequent send after delivery runs it again.

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;
#[path = "../../../dev/test_utils/smp.rs"]
mod smp;

use std::hint::spin_loop;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const VECTOR: u8 = 42;
static CPU1_DISABLED: AtomicBool = AtomicBool::new(false);
static TWO_SENT: AtomicBool = AtomicBool::new(false);
static HANDLED_COUNT: AtomicUsize = AtomicUsize::new(0);
static FIRST_DELIVERED: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);

extern "Rust" fn handler(vector: u8) {
    assert_eq!(vector, VECTOR);
    assert_eq!(smp::current(), 1);
    HANDLED_COUNT.fetch_add(1, Ordering::Release);
}

fn cpu1_entry(_: usize) {
    irq::disable();
    CPU1_DISABLED.store(true, Ordering::Release);
    while !TWO_SENT.load(Ordering::Acquire) {
        spin_loop();
    }
    irq::enable();
    while HANDLED_COUNT.load(Ordering::Acquire) == 0 {
        spin_loop();
    }
    for _ in 0..8 {
        spin_loop();
    }
    assert_eq!(HANDLED_COUNT.load(Ordering::Acquire), 1);
    FIRST_DELIVERED.store(true, Ordering::Release);

    while !STOP.load(Ordering::Acquire) {
        spin_loop();
    }
}

fn main() {
    irq::register_entry(handler);
    smp::start(1, cpu1_entry, 0);
    while !CPU1_DISABLED.load(Ordering::Acquire) {
        spin_loop();
    }
    irq::send_ipi(1, VECTOR);
    irq::send_ipi(1, VECTOR);
    TWO_SENT.store(true, Ordering::Release);

    while !FIRST_DELIVERED.load(Ordering::Acquire) {
        spin_loop();
    }
    irq::send_ipi(1, VECTOR);
    while HANDLED_COUNT.load(Ordering::Acquire) < 2 {
        spin_loop();
    }
    assert_eq!(HANDLED_COUNT.load(Ordering::Acquire), 2);
    STOP.store(true, Ordering::Release);
}
