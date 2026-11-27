//@compile-flags: -Zkmiri-toml=dev/test_utils/irq-masked-at-start.toml
//! With `enabled_at_start = false`, every CPU starts with interrupts masked, as
//! hardware leaves reset, until the program enables them on that CPU.

#[path = "../../../dev/test_utils/smp.rs"]
mod smp;

use std::hint::spin_loop;
use std::sync::atomic::{AtomicUsize, Ordering};

unsafe extern "Rust" {
    fn kern_miri_irq_is_enabled() -> bool;
    fn kern_miri_irq_enable();
}

/// 0 = not yet seen, 1 = masked, 2 = enabled.
static SECONDARY: AtomicUsize = AtomicUsize::new(0);

fn entry(_arg: usize) {
    let enabled = unsafe { kern_miri_irq_is_enabled() };
    SECONDARY.store(if enabled { 2 } else { 1 }, Ordering::Release);
}

fn main() {
    unsafe {
        assert!(!kern_miri_irq_is_enabled());
        kern_miri_irq_enable();
        assert!(kern_miri_irq_is_enabled());
    }
    smp::start(1, entry, 0);
    while SECONDARY.load(Ordering::Acquire) == 0 {
        spin_loop();
    }
    assert_eq!(SECONDARY.load(Ordering::Acquire), 1);
}
