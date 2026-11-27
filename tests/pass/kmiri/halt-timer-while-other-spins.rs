//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2-irq-rand-timer.toml
//! A halted CPU still receives timer IRQs while another CPU is spinning.

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;
#[path = "../../../dev/test_utils/smp.rs"]
mod smp;

use std::sync::atomic::{AtomicBool, Ordering};

static CPU1_TIMER_RAN: AtomicBool = AtomicBool::new(false);
static DONE: AtomicBool = AtomicBool::new(false);

extern "Rust" fn handler(vector: u8) {
    if vector == 32 && smp::current() == 1 {
        CPU1_TIMER_RAN.store(true, Ordering::Release);
    }
}

fn cpu1_entry(_: usize) {
    while !CPU1_TIMER_RAN.load(Ordering::Acquire) {
        irq::halt();
    }
    DONE.store(true, Ordering::Release);
}

fn main() {
    irq::register_entry(handler);
    irq::set_timer_vector(32);
    smp::start(1, cpu1_entry, 0);
    while !DONE.load(Ordering::Acquire) {
        std::hint::spin_loop();
    }
}
