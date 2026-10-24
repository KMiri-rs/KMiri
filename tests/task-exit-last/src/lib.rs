#![no_std]
#![feature(format_args_nl)]

extern crate alloc;

use core::sync::atomic::{AtomicBool, Ordering};
use ostd::task::{Task, TaskOptions};

static COMPLETED: AtomicBool = AtomicBool::new(false);

// Keep the no-successor exit case working while fixing task handoff (#277).
// One spawned task returns with an empty runqueue; its Miri entry must finish.
// Expected: completion is observed and process exit passes without ignore-leaks.
#[ostd::ktest::miri_main]
fn kernel_main() {
    let _task = TaskOptions::new(|| {
        COMPLETED.store(true, Ordering::SeqCst);
    })
    .spawn()
    .unwrap();

    Task::yield_now();
    assert!(COMPLETED.load(Ordering::SeqCst));
    ostd::miri_println!("task-exit-last: ok");
}
