//! A use-after-free of a task that the bootstrap context switched to must be
//! reported.
//!
//! Safe code turns `CurrentTask` into a `&'static Task` that outlives the task
//! (Asterinas issue 3897). The bootstrap context switches to task A. A leaks
//! its `CurrentTask`, hands the derived `&'static Task` to task B and exits;
//! B then runs holding a reference to A's freed `Task`. Without a driver task
//! the only context that switched to A is the bootstrap context, which never
//! resumes, so nothing but A's own exit may keep A alive.
//!
//! Expected: Undefined Behavior (use-after-free) when B's closure is called.
#![no_std]
#![deny(unsafe_code)]
#![cfg_attr(miri, feature(format_args_nl))]

extern crate alloc;

use alloc::boxed::Box;
use core::sync::atomic::{AtomicBool, Ordering::SeqCst};
use ostd::task::{CurrentTask, Task, TaskOptions};

// Diagnostics only; compiled out outside Miri.
macro_rules! note {
    ($($t:tt)*) => { #[cfg(miri)] ostd::miri_println!($($t)*); };
}

static A_FREED: AtomicBool = AtomicBool::new(false);
static B_DONE: AtomicBool = AtomicBool::new(false);

/// Stored as task A's `data`, so it is dropped exactly when A's `Task` is freed.
struct Witness;

impl Drop for Witness {
    fn drop(&mut self) {
        A_FREED.store(true, SeqCst);
    }
}

#[cfg_attr(miri, ostd::ktest::miri_main)]
#[cfg_attr(not(miri), ostd::main)]
fn kernel_main() {
    // Variant without a driver task: the bootstrap context switches into A.
    {
        let a = TaskOptions::new(|| {
            let leaked: &'static mut CurrentTask = Box::leak(Box::new(Task::current().unwrap()));
            let me: &'static Task = &**leaked;

            let b = TaskOptions::new(move || {
                // Runs after task A has exited and its `Task` has been freed.
                let _cpu = me.schedule_info().cpu.get();
                B_DONE.store(true, SeqCst);
            })
            .spawn()
            .unwrap();
            drop(b);
            // Task A returns and exits here.
        })
        .data(Witness)
        .spawn()
        .unwrap();
        drop(a);

        let mut reported = false;
        while !B_DONE.load(SeqCst) {
            if !reported && A_FREED.load(SeqCst) {
                note!("current-task-uaf-nodriver: task A's Task freed; task B not started yet");
                reported = true;
            }
            Task::yield_now();
        }
    }
}
