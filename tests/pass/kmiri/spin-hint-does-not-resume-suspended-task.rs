//@compile-flags: -Zkmiri-toml=dev/test_utils/no-paging.toml
//! One CPU. A task that was created but never switched to must not run when
//! the running context spins: only a switch by the kernel resumes it.

use std::hint::spin_loop;
use std::sync::atomic::{AtomicBool, Ordering};

static RAN: AtomicBool = AtomicBool::new(false);

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
    RAN.store(true, Ordering::SeqCst);
}

fn main() {
    let thread = unsafe { miri_create_new_thread(task, 0, 0x7000000, 0x100000) };
    for _ in 0..200 {
        spin_loop();
    }
    assert!(!RAN.load(Ordering::SeqCst));
    unsafe { miri_switch_to(thread) };
    assert!(RAN.load(Ordering::SeqCst));
}
