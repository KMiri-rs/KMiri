//@compile-flags: -Zkmiri-toml=dev/test_utils/clint.toml -Zmiri-permissive-provenance
//! Writing another CPU's `msip` wakes it from halt with a software interrupt,
//! pending until its handler writes `msip` back to 0.

#[path = "../../../dev/test_utils/clint.rs"]
mod clint;
#[path = "../../../dev/test_utils/irq.rs"]
mod irq;
#[path = "../../../dev/test_utils/smp.rs"]
mod smp;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

static HALTING: AtomicBool = AtomicBool::new(false);
static ENTRIES: AtomicUsize = AtomicUsize::new(0);

extern "Rust" fn handler(vector: u8) {
    assert_eq!(vector, clint::SOFTWARE);
    assert_eq!(smp::current(), 1);
    assert_eq!(clint::msip(1), 1);
    clint::set_msip(1, 0);
    ENTRIES.fetch_add(1, Ordering::SeqCst);
}

fn cpu1(_: usize) {
    irq::register_entry(handler);
    HALTING.store(true, Ordering::SeqCst);
    irq::halt();
    assert_eq!(ENTRIES.load(Ordering::SeqCst), 1);
}

fn main() {
    smp::start(1, cpu1, 0);
    while !HALTING.load(Ordering::SeqCst) {
        std::hint::spin_loop();
    }
    clint::set_msip(1, 1);
    while ENTRIES.load(Ordering::SeqCst) == 0 {
        std::hint::spin_loop();
    }
}
