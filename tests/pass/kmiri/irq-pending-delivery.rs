//@compile-flags: -Zkmiri-toml=dev/test_utils/irq.toml

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;

const TIMER_VECTOR: u8 = 40;

use std::sync::atomic::{AtomicUsize, Ordering};

const WRITER: usize = 1 << (usize::BITS - 1);
static LOCK: AtomicUsize = AtomicUsize::new(0);
static HANDLER_RUNS: AtomicUsize = AtomicUsize::new(0);

struct ReadGuard;

impl ReadGuard {
    fn acquire() -> Self {
        let old = LOCK.fetch_add(1, Ordering::Acquire);
        assert_eq!(old & WRITER, 0);
        Self
    }
}

impl Drop for ReadGuard {
    fn drop(&mut self) {
        LOCK.fetch_sub(1, Ordering::Release);
    }
}

fn write_lock() {
    loop {
        if LOCK
            .compare_exchange(0, WRITER, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            return;
        }
        std::hint::spin_loop();
    }
}

extern "Rust" fn timer_irq_handler() {
    write_lock();
    HANDLER_RUNS.fetch_add(1, Ordering::SeqCst);
    LOCK.store(0, Ordering::Release);
}

extern "Rust" fn entry(vector: u8) {
    assert_eq!(vector, TIMER_VECTOR);
    timer_irq_handler();
}

fn main() {
    irq::register_entry(entry);
    let outer_irq = irq::IrqGuard::new();
    let inner_irq = irq::IrqGuard::new();
    let reader = ReadGuard::acquire();

    irq::raise(TIMER_VECTOR);
    assert_eq!(HANDLER_RUNS.load(Ordering::SeqCst), 0);

    drop(reader);
    drop(inner_irq);
    assert_eq!(HANDLER_RUNS.load(Ordering::SeqCst), 0);

    drop(outer_irq);
    assert_eq!(HANDLER_RUNS.load(Ordering::SeqCst), 1);
    assert_eq!(LOCK.load(Ordering::Acquire), 0);
}
