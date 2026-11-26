//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2-irq.toml
//! An IPI sent from CPU 0 to CPU 1 is delivered to the registered IRQ entry on CPU 1.

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;
#[path = "../../../dev/test_utils/smp.rs"]
mod smp;

use std::hint::spin_loop;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const VECTOR: u8 = 40;
static CPU1_READY: AtomicBool = AtomicBool::new(false);
static HANDLED: AtomicBool = AtomicBool::new(false);
static HANDLED_CPU: AtomicUsize = AtomicUsize::new(usize::MAX);
static STOP: AtomicBool = AtomicBool::new(false);

extern "Rust" fn handler(vector: u8) {
    assert_eq!(vector, VECTOR);
    HANDLED_CPU.store(smp::current(), Ordering::Relaxed);
    HANDLED.store(true, Ordering::Release);
}

fn cpu1_entry(_: usize) {
    irq::enable();
    CPU1_READY.store(true, Ordering::Release);
    while !STOP.load(Ordering::Acquire) {
        spin_loop();
    }
}

fn main() {
    irq::register_entry(handler);
    smp::start(1, cpu1_entry, 0);
    while !CPU1_READY.load(Ordering::Acquire) {
        spin_loop();
    }
    irq::send_ipi(1, VECTOR);
    while !HANDLED.load(Ordering::Acquire) {
        spin_loop();
    }
    assert_eq!(HANDLED_CPU.load(Ordering::Relaxed), 1);
    STOP.store(true, Ordering::Release);
}
