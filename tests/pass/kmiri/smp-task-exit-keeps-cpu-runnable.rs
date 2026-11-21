//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2-no-switch.toml
//! A task on CPU 0 returns to the bootstrap context. CPU 0 must still count as
//! a running CPU afterwards: a spin-loop hint on CPU 1 hands it control again.

#[path = "../../../dev/test_utils/smp.rs"]
mod smp;

use std::hint::spin_loop;
use std::sync::atomic::{AtomicBool, Ordering};

static STARTED: AtomicBool = AtomicBool::new(false);
static GO: AtomicBool = AtomicBool::new(false);
static DONE: AtomicBool = AtomicBool::new(false);

unsafe extern "Rust" {
    fn miri_create_new_thread(
        func: fn(usize),
        arg: usize,
        stack_end: usize,
        stack_size: usize,
    ) -> usize;
    fn miri_switch_to(thread: usize);
}

fn cpu1(_: usize) {
    STARTED.store(true, Ordering::Release);
    let mut spins = 0;
    while !GO.load(Ordering::Acquire) {
        spin_loop();
        spins += 1;
        assert!(spins < 1000, "CPU 0 never ran again");
    }
    DONE.store(true, Ordering::Release);
}

fn task(_: usize) {
    while !STARTED.load(Ordering::Acquire) {
        spin_loop();
    }
}

fn main() {
    smp::start(1, cpu1, 0);
    let thread = unsafe { miri_create_new_thread(task, 0, 0x6000000, 0x100000) };
    unsafe { miri_switch_to(thread) };
    // Back in the bootstrap context on CPU 0; let CPU 1 run before releasing it.
    for _ in 0..10 {
        spin_loop();
    }
    GO.store(true, Ordering::Release);
    while !DONE.load(Ordering::Acquire) {
        spin_loop();
    }
}
