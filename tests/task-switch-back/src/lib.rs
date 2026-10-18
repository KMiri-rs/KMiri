#![no_std]
#![feature(format_args_nl)]

extern crate alloc;

use ostd::task::{Task, TaskOptions};

// Regression test for https://github.com/KMiri-rs/KMiri/issues/265.
//
// When a task runs for the first time, OSTD must call `after_switching_to`,
// which clears `switched_to_cpu` of the task it switched away from. Otherwise
// switching back to that task spins forever in `before_switching_to` and
// prints `WARN: Switching to a task already running in the foreground`.
//
// `kernel_main` itself is not a task, so the switches happen between two
// spawned tasks: `a` starts `b`, and `b` switches back to `a`.
//
// A task that exits while another task is runnable keeps its Miri thread, so
// run this test with `MIRIFLAGS=-Zmiri-ignore-leaks`.
//
// Expected: no WARN, and the kernel prints `b: resumed` and `a: resumed`.
#[ostd::ktest::miri_main]
fn kernel_main() {
    let _a = TaskOptions::new(|| {
        let _b = TaskOptions::new(|| {
            // Switch back to `a`, which is still marked as running if the
            // first switch to `b` did not clean up.
            Task::yield_now();
            ostd::miri_println!("b: resumed");
        })
        .spawn()
        .unwrap();

        // Run `b` for the first time.
        Task::yield_now();
        ostd::miri_println!("a: resumed");
    })
    .spawn()
    .unwrap();

    Task::yield_now();
}
