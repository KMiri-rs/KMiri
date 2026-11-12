//@compile-flags: -Zkmiri-toml=dev/test_utils/irq-timer.toml

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;

const TIMER_VECTOR: u8 = 40;

use std::sync::atomic::{AtomicUsize, Ordering};

static TIMER_RUNS: AtomicUsize = AtomicUsize::new(0);

extern "Rust" fn timer_handler() {
    TIMER_RUNS.fetch_add(1, Ordering::SeqCst);
}

extern "Rust" fn entry(vector: u8) {
    assert_eq!(vector, TIMER_VECTOR);
    timer_handler();
}

fn main() {
    irq::register_entry(entry);
    irq::set_timer_vector(TIMER_VECTOR);
    while TIMER_RUNS.load(Ordering::SeqCst) == 0 {
        std::hint::spin_loop();
    }
    assert_eq!(TIMER_RUNS.load(Ordering::SeqCst), 1);
}
