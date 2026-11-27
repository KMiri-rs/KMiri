//@compile-flags: -Zkmiri-toml=dev/test_utils/irq-timer.toml
//! A single halted CPU wakes when the periodic timer fires.

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;

use std::sync::atomic::{AtomicBool, Ordering};

static TIMER_RAN: AtomicBool = AtomicBool::new(false);

extern "Rust" fn handler(vector: u8) {
    if vector == 32 {
        TIMER_RAN.store(true, Ordering::Release);
    }
}

fn main() {
    irq::register_entry(handler);
    irq::set_timer_vector(32);
    irq::halt();
    assert!(TIMER_RAN.load(Ordering::Acquire));
}
