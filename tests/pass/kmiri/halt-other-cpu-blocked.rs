//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2-irq.toml
//! A halted CPU must not run while the only other CPU is blocked (here: sleeping).
//! It wakes on the IPI that the other CPU sends once it runs again.

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;
#[path = "../../../dev/test_utils/smp.rs"]
mod smp;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static CPU0_HALTING: AtomicBool = AtomicBool::new(false);
static SENT: AtomicBool = AtomicBool::new(false);
static HANDLED: AtomicBool = AtomicBool::new(false);

extern "Rust" fn handler(_vector: u8) {
    HANDLED.store(true, Ordering::Release);
}

fn cpu1_entry(_: usize) {
    while !CPU0_HALTING.load(Ordering::Acquire) {
        std::hint::spin_loop();
    }
    std::thread::sleep(Duration::from_millis(10));
    SENT.store(true, Ordering::Release);
    irq::send_ipi(0, 40);
}

fn main() {
    irq::register_entry(handler);
    smp::start(1, cpu1_entry, 0);
    CPU0_HALTING.store(true, Ordering::Release);
    irq::halt();
    assert!(SENT.load(Ordering::Acquire), "CPU 0 ran while halted, before CPU 1 sent the IPI");
    assert!(HANDLED.load(Ordering::Acquire));
}
