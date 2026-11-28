//@compile-flags: -Zkmiri-toml=dev/test_utils/clint.toml -Zmiri-permissive-provenance
//! When every CPU is halted, time jumps to the earliest timer interrupt
//! instead of running a billion basic blocks.

#[path = "../../../dev/test_utils/clint.rs"]
mod clint;
#[path = "../../../dev/test_utils/irq.rs"]
mod irq;

use std::sync::atomic::{AtomicBool, Ordering};

static RAN: AtomicBool = AtomicBool::new(false);

extern "Rust" fn handler(vector: u8) {
    assert_eq!(vector, clint::TIMER);
    clint::set_mtimecmp(0, u64::MAX);
    RAN.store(true, Ordering::SeqCst);
}

fn main() {
    irq::register_entry(handler);
    let deadline = clint::mtime() + 1_000_000_000;
    clint::set_mtimecmp(0, deadline);
    irq::halt();
    assert!(RAN.load(Ordering::SeqCst));
    assert!(clint::mtime() >= deadline);
}
