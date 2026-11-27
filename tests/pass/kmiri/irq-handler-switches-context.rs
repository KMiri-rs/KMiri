//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2-irq.toml
//! A handler that switches to another kernel context, as a kernel that preempts
//! on interrupt return does. The CPU then runs that context, which takes an
//! interrupt of its own, and the interrupted context finishes its handler when
//! it is switched back to.

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;

use std::sync::atomic::{AtomicUsize, Ordering};

unsafe extern "Rust" {
    fn miri_create_new_thread(
        func: fn(usize),
        arg: usize,
        stack_end: usize,
        stack_size: usize,
    ) -> usize;
    fn miri_switch_to(thread: usize);
    fn miri_switch_to_after_return(thread: usize);
}

static A: AtomicUsize = AtomicUsize::new(0);
static B: AtomicUsize = AtomicUsize::new(0);
static STEP: AtomicUsize = AtomicUsize::new(0);

fn step(from: usize) {
    assert_eq!(STEP.compare_exchange(from, from + 1, Ordering::SeqCst, Ordering::SeqCst), Ok(from));
}

extern "Rust" fn handler(vector: u8) {
    match vector {
        40 => {
            // On A.
            step(1);
            unsafe { miri_switch_to(B.load(Ordering::SeqCst)) };
            // A again, still inside this handler.
            step(4);
        }
        41 => step(2), // On B.
        _ => unreachable!(),
    }
}

fn a(_: usize) {
    irq::enable();
    step(0);
    irq::raise(40);
    std::hint::black_box(());
    step(5);
    unsafe { miri_switch_to_after_return(B.load(Ordering::SeqCst)) };
}

fn b(_: usize) {
    // A is inside its handler, but this CPU now runs B, which is not.
    irq::enable();
    irq::raise(41);
    std::hint::black_box(());
    step(3);
    unsafe { miri_switch_to(A.load(Ordering::SeqCst)) };
    step(6);
}

fn main() {
    irq::register_entry(handler);
    unsafe {
        A.store(miri_create_new_thread(a, 0, 0x7000000, 0x100000), Ordering::SeqCst);
        B.store(miri_create_new_thread(b, 0, 0x7200000, 0x100000), Ordering::SeqCst);
        miri_switch_to(A.load(Ordering::SeqCst));
    }
    assert_eq!(STEP.load(Ordering::SeqCst), 7);
}
