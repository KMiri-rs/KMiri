#![no_std]
#![feature(format_args_nl)]

extern crate alloc;

use core::sync::atomic::{AtomicUsize, Ordering};
use ostd::task::{Task, TaskOptions};

static COMPLETED: AtomicUsize = AtomicUsize::new(0);

// Regression for KMiri-rs/KMiri#277: `a` exits while `b` remains runnable.
// Both Miri entries must return, not leave `a` suspended in a context switch.
// Expected: two completions and clean process exit without ignore-leaks.
#[ostd::ktest::miri_main]
fn kernel_main() {
    let _a = TaskOptions::new(|| {
        let _b = TaskOptions::new(|| {
            COMPLETED.fetch_add(1, Ordering::SeqCst);
            ostd::miri_println!("b: done");
        })
        .spawn()
        .unwrap();
        COMPLETED.fetch_add(1, Ordering::SeqCst);
        ostd::miri_println!("a: done");
    })
    .spawn()
    .unwrap();

    Task::yield_now();
    assert_eq!(COMPLETED.load(Ordering::SeqCst), 2);
    ostd::miri_println!("task-exit-with-runnable: ok");
}
