//@compile-flags: -Zkmiri-toml=dev/test_utils/irq.toml
//! Halting with a pending IRQ enables IF and returns immediately so the IRQ is delivered.

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;

use std::sync::atomic::{AtomicBool, Ordering};

static HANDLED: AtomicBool = AtomicBool::new(false);

extern "Rust" fn handler(vector: u8) {
    assert_eq!(vector, 40);
    HANDLED.store(true, Ordering::Release);
}

fn main() {
    irq::register_entry(handler);
    irq::disable();
    irq::raise(40);
    irq::halt();
    assert!(HANDLED.load(Ordering::Acquire));
}
