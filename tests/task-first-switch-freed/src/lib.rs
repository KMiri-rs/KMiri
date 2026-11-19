//! A task switched to from the bootstrap context must be freed when it exits.
//!
//! The bootstrap context switches to task A and stays suspended while tasks
//! run. A spawns B and exits. On hardware A's `Task` is freed once B has
//! switched in (B releases the previous task), because the suspended
//! bootstrap frame holds no reference to A. KMiri must not keep A alive from
//! the suspended frame either.
//!
//! Expected: B observes that A was freed, and the kernel exits normally.
#![no_std]
#![deny(unsafe_code)]
#![cfg_attr(miri, feature(format_args_nl))]

extern crate alloc;

use core::sync::atomic::{AtomicBool, Ordering::SeqCst};
use ostd::task::{Task, TaskOptions};

// Diagnostics only; compiled out outside Miri.
macro_rules! note {
    ($($t:tt)*) => { #[cfg(miri)] ostd::miri_println!($($t)*); };
}

static A_FREED: AtomicBool = AtomicBool::new(false);

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
    let a = TaskOptions::new(|| {
        let b = TaskOptions::new(|| {
            for _ in 0..64 {
                if A_FREED.load(SeqCst) {
                    break;
                }
                Task::yield_now();
            }
            assert!(
                A_FREED.load(SeqCst),
                "task A, switched to from the bootstrap context, was not freed"
            );
            note!("task-first-switch-freed: task A freed after it exited");
        })
        .spawn()
        .unwrap();
        drop(b);
        // Task A exits here.
    })
    .data(Witness)
    .spawn()
    .unwrap();
    drop(a);
    // Returning lets the bootstrap context switch to A.
}
