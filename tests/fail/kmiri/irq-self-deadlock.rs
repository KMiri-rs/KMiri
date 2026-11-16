//@compile-flags: -Zkmiri-toml=dev/test_utils/irq.toml
//@error-in-other-file: IRQ watchdog: possible self-deadlock
// The watchdog still catches self-deadlock, but its top frame depends on injection phase.

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;

const TIMER_VECTOR: u8 = 40;

use std::sync::atomic::{AtomicUsize, Ordering};

const WRITER: usize = 1 << (usize::BITS - 1);
static LOCK: AtomicUsize = AtomicUsize::new(0);

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
            LOCK.store(0, Ordering::Release);
            return;
        }
        std::hint::spin_loop();
    }
}

extern "Rust" fn timer_irq_handler() {
    write_lock();
}

extern "Rust" fn entry(vector: u8) {
    assert_eq!(vector, TIMER_VECTOR);
    timer_irq_handler();
}

fn main() {
    irq::register_entry(entry);
    let _reader = ReadGuard::acquire();
    // The queued handler runs at the next MIR safe point while the reader is held.
    irq::raise(TIMER_VECTOR);
}
