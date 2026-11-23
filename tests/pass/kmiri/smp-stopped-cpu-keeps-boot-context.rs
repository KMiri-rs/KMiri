//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2-no-switch.toml
//! CPU 1 switches from its boot context to a task, and the task returns with no
//! successor, so CPU 1 stops. Its boot context is still suspended in the switch
//! when the program ends; like a CPU that is still running, that is not a leak.

#[path = "../../../dev/test_utils/smp.rs"]
mod smp;

use std::hint::spin_loop;
use std::sync::atomic::{AtomicBool, Ordering};

static TASK_DONE: AtomicBool = AtomicBool::new(false);

unsafe extern "Rust" {
    fn miri_create_new_thread(
        func: fn(usize),
        arg: usize,
        stack_end: usize,
        stack_size: usize,
    ) -> usize;
    fn miri_switch_to(thread: usize);
}

fn task(_: usize) {
    assert_eq!(smp::current(), 1);
    TASK_DONE.store(true, Ordering::Release);
}

fn cpu1(_: usize) {
    let thread = unsafe { miri_create_new_thread(task, 0, 0x6000000, 0x100000) };
    unsafe { miri_switch_to(thread) };
    unreachable!("CPU 1 stops when its task returns with no successor");
}

fn main() {
    smp::start(1, cpu1, 0);
    while !TASK_DONE.load(Ordering::Acquire) {
        spin_loop();
    }
}
