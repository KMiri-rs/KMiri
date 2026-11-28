//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2-irq.toml
//! A context switched away inside its handler on one CPU is switched back to
//! on another CPU and returns from the interrupt there. On x86 the return
//! restores the interrupt flag of the CPU it returns on; elsewhere the flag
//! stays as the program left it, off (see `irq-return-interrupt-flag.rs`).

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;
#[path = "../../../dev/test_utils/smp.rs"]
mod smp;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

unsafe extern "Rust" {
    fn miri_create_new_thread(
        func: fn(usize),
        arg: usize,
        stack_end: usize,
        stack_size: usize,
    ) -> usize;
    fn miri_switch_to(thread: usize);
    fn kern_miri_irq_is_enabled() -> bool;
}

static A: AtomicUsize = AtomicUsize::new(0);
static OTHER: AtomicUsize = AtomicUsize::new(0);
static A_SWITCHED_AWAY: AtomicBool = AtomicBool::new(false);
static DONE: AtomicBool = AtomicBool::new(false);

extern "Rust" fn handler(_vector: u8) {
    assert_eq!(smp::current(), 0);
    unsafe { miri_switch_to(OTHER.load(Ordering::SeqCst)) };
    // Switched back to by CPU 1, still inside the handler.
    assert_eq!(smp::current(), 1);
    assert!(!unsafe { kern_miri_irq_is_enabled() });
}

fn a(_: usize) {
    irq::enable();
    irq::raise(40);
    std::hint::black_box(());
    // Returned from the interrupt on CPU 1.
    assert_eq!(smp::current(), 1);
    let x86 = cfg!(any(target_arch = "x86", target_arch = "x86_64"));
    assert_eq!(unsafe { kern_miri_irq_is_enabled() }, x86);
    DONE.store(true, Ordering::SeqCst);
}

/// Runs on CPU 0 once A is switched away inside its handler.
fn other(_: usize) {
    A_SWITCHED_AWAY.store(true, Ordering::SeqCst);
    while !DONE.load(Ordering::SeqCst) {
        std::hint::spin_loop();
    }
}

fn cpu1(_: usize) {
    while !A_SWITCHED_AWAY.load(Ordering::SeqCst) {
        std::hint::spin_loop();
    }
    unsafe { miri_switch_to(A.load(Ordering::SeqCst)) };
}

fn main() {
    irq::register_entry(handler);
    unsafe {
        A.store(miri_create_new_thread(a, 0, 0x6000000, 0x100000), Ordering::SeqCst);
        OTHER.store(miri_create_new_thread(other, 0, 0x6200000, 0x100000), Ordering::SeqCst);
    }
    smp::start(1, cpu1, 0);
    unsafe { miri_switch_to(A.load(Ordering::SeqCst)) };
    assert!(DONE.load(Ordering::SeqCst));
}
