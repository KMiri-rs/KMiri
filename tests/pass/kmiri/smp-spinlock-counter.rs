//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2.toml
//! Two CPUs update a plain counter under a spin lock: no data race, no lost update.

#[path = "../../../dev/test_utils/smp.rs"]
mod smp;

use std::cell::UnsafeCell;
use std::hint::spin_loop;
use std::sync::atomic::{AtomicBool, Ordering};

const ROUNDS: usize = 100;

struct Plain(UnsafeCell<usize>);
unsafe impl Sync for Plain {}

static LOCK: AtomicBool = AtomicBool::new(false);
static COUNTER: Plain = Plain(UnsafeCell::new(0));
static DONE: AtomicBool = AtomicBool::new(false);

fn add_rounds() {
    for _ in 0..ROUNDS {
        while LOCK.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            spin_loop();
        }
        unsafe { *COUNTER.0.get() += 1 };
        LOCK.store(false, Ordering::Release);
    }
}

fn entry(_: usize) {
    add_rounds();
    DONE.store(true, Ordering::Release);
}

fn main() {
    smp::start(1, entry, 0);
    add_rounds();
    while !DONE.load(Ordering::Acquire) {
        spin_loop();
    }
    assert_eq!(unsafe { *COUNTER.0.get() }, 2 * ROUNDS);
}
