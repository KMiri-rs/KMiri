//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2-irq.toml
//! While CPU 0 runs an interrupt handler, CPU 1 keeps running: the handler can
//! wait for something CPU 1 does.

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;
#[path = "../../../dev/test_utils/smp.rs"]
mod smp;

use std::hint::spin_loop;
use std::sync::atomic::{AtomicBool, Ordering};

const VECTOR: u8 = 40;
static IN_HANDLER: AtomicBool = AtomicBool::new(false);
static GO: AtomicBool = AtomicBool::new(false);

extern "Rust" fn handler(vector: u8) {
    assert_eq!(vector, VECTOR);
    assert_eq!(smp::current(), 0);
    IN_HANDLER.store(true, Ordering::Release);
    while !GO.load(Ordering::Acquire) {
        spin_loop();
    }
}

fn entry(_: usize) {
    while !IN_HANDLER.load(Ordering::Acquire) {
        spin_loop();
    }
    GO.store(true, Ordering::Release);
}

fn main() {
    irq::register_entry(handler);
    smp::start(1, entry, 0);
    irq::raise(VECTOR);
    assert!(GO.load(Ordering::Acquire));
}
