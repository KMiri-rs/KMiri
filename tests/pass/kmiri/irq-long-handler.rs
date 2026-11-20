//@compile-flags: -Zkmiri-toml=dev/test_utils/irq-timer.toml
//! An interrupt handler may run for as long as it needs to: KMiri puts no limit
//! on the number of basic blocks a handler executes.

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;

use std::sync::atomic::{AtomicUsize, Ordering};

const VECTOR: u8 = 40;
const ROUNDS: usize = 20_000;
static WORK: AtomicUsize = AtomicUsize::new(0);

extern "Rust" fn entry(vector: u8) {
    assert_eq!(vector, VECTOR);
    // Every round is at least one basic block.
    for _ in 0..ROUNDS {
        WORK.fetch_add(1, Ordering::Relaxed);
    }
}

fn main() {
    irq::register_entry(entry);
    irq::raise(VECTOR);
    assert_eq!(WORK.load(Ordering::Relaxed), ROUNDS);
}
