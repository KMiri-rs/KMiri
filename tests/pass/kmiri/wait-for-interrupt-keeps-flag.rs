//@compile-flags: -Zkmiri-toml=dev/test_utils/clint.toml -Zmiri-permissive-provenance
//! `kern_miri_wait_for_interrupt` waits like RISC-V `wfi`: a pending interrupt
//! wakes the CPU even with interrupts off, and it is taken only once they are
//! turned on.

#[path = "../../../dev/test_utils/clint.rs"]
mod clint;
#[path = "../../../dev/test_utils/irq.rs"]
mod irq;

use std::sync::atomic::{AtomicBool, Ordering};

unsafe extern "Rust" {
    fn kern_miri_wait_for_interrupt();
    fn kern_miri_irq_is_enabled() -> bool;
}

static RAN: AtomicBool = AtomicBool::new(false);

extern "Rust" fn handler(vector: u8) {
    assert_eq!(vector, clint::TIMER);
    clint::set_mtimecmp(0, u64::MAX);
    RAN.store(true, Ordering::SeqCst);
}

fn main() {
    irq::register_entry(handler);
    irq::disable();
    let deadline = clint::mtime() + 1_000_000;
    clint::set_mtimecmp(0, deadline);
    unsafe { kern_miri_wait_for_interrupt() };
    // Woken by the pending timer interrupt, which was not taken.
    assert!(clint::mtime() >= deadline);
    assert!(!unsafe { kern_miri_irq_is_enabled() });
    assert!(!RAN.load(Ordering::SeqCst));
    irq::enable();
    std::hint::black_box(());
    assert!(RAN.load(Ordering::SeqCst));
}
