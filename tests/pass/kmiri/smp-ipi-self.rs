//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2-irq.toml
//! Sending an IPI to the current CPU delivers it on the current CPU.

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;
#[path = "../../../dev/test_utils/smp.rs"]
mod smp;

use std::sync::atomic::{AtomicUsize, Ordering};

const VECTOR: u8 = 44;
static HANDLED_CPU: AtomicUsize = AtomicUsize::new(usize::MAX);

extern "Rust" fn handler(vector: u8) {
    assert_eq!(vector, VECTOR);
    HANDLED_CPU.store(smp::current(), Ordering::Relaxed);
}

fn main() {
    irq::register_entry(handler);
    irq::send_ipi(0, VECTOR);
    assert_eq!(HANDLED_CPU.load(Ordering::Relaxed), 0);
}
