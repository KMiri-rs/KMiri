//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2-irq.toml
//! A write after `send_ipi` is not ordered before the IPI handler on the target
//! CPU, so a concurrent write in the handler is a data race.

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;
#[path = "../../../dev/test_utils/smp.rs"]
mod smp;

use std::hint::spin_loop;
use std::sync::atomic::{AtomicBool, Ordering};

const VECTOR: u8 = 45;
static mut SHARED: usize = 0;
static CPU1_READY: AtomicBool = AtomicBool::new(false);
static HANDLED: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);

extern "Rust" fn handler(vector: u8) {
    assert_eq!(vector, VECTOR);
    unsafe {
        SHARED = 2; //~ ERROR: Data race detected
    }
    HANDLED.store(true, Ordering::Relaxed);
}

fn cpu1_entry(_: usize) {
    irq::enable();
    CPU1_READY.store(true, Ordering::Release);
    while !STOP.load(Ordering::Acquire) {
        spin_loop();
    }
}

fn main() {
    irq::register_entry(handler);
    smp::start(1, cpu1_entry, 0);
    while !CPU1_READY.load(Ordering::Acquire) {
        spin_loop();
    }
    irq::send_ipi(1, VECTOR);
    unsafe {
        SHARED = 1;
    }
    while !HANDLED.load(Ordering::Relaxed) {
        spin_loop();
    }
    STOP.store(true, Ordering::Release);
}
