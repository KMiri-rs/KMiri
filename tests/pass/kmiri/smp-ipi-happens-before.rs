//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2-irq.toml
//! Writes before `send_ipi` happen-before the IPI handler on the target CPU.

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;
#[path = "../../../dev/test_utils/smp.rs"]
mod smp;

use std::hint::spin_loop;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const VECTOR: u8 = 43;
static mut PAYLOAD: usize = 0;
static CPU1_READY: AtomicBool = AtomicBool::new(false);
static OBSERVED: AtomicUsize = AtomicUsize::new(0);
static HANDLED: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);

extern "Rust" fn handler(vector: u8) {
    assert_eq!(vector, VECTOR);
    let val = unsafe { PAYLOAD };
    OBSERVED.store(val, Ordering::Relaxed);
    HANDLED.store(true, Ordering::Release);
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
    unsafe {
        PAYLOAD = 0x1234_5678;
    }
    irq::send_ipi(1, VECTOR);
    while !HANDLED.load(Ordering::Acquire) {
        spin_loop();
    }
    assert_eq!(OBSERVED.load(Ordering::Relaxed), 0x1234_5678);
    STOP.store(true, Ordering::Release);
}
