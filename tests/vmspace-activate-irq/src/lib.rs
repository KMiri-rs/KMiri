//! `VmSpace::activate` disables preemption but leaves local IRQs enabled. It
//! loads the CPU's activated-VM-space pointer, later stores its own, and then
//! releases the old one with `Arc::from_raw`. A timer callback that also calls
//! `activate` can run between that load and that store; the callback and the
//! interrupted `activate` then both release the same old pointer, so one
//! `Arc<VmSpace>` drops a count that a holder still owns and is freed while in
//! use.
//!
//! The program is safe code calling public OSTD interfaces only. Timer
//! interrupts are injected by `kmiri.toml` in this directory.
//!
//! Expected: Undefined Behavior in `VmSpace::activate`. With the injection
//! config here it is a use-after-free of a `VmSpace` `Arc`. Other injection
//! points expose the same double release differently: a Stacked Borrows
//! protector error when the callback reads a `VmSpace` that the interrupted
//! `activate` is dropping, or OSTD's own reference-count assertion on the
//! freed space's root page-table frame (a kernel panic, not a Miri report).
//! The check accepts a use-after-free or a Stacked Borrows protector report
//! inside `VmSpace::activate`.
#![no_std]
#![deny(unsafe_code)]
#![feature(format_args_nl)]

extern crate alloc;

use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};
use ostd::mm::VmSpace;

const ROUNDS: usize = 400;

static CALLBACK_RUNS: AtomicUsize = AtomicUsize::new(0);

#[ostd::ktest::miri_main]
fn kernel_main() {
    let t = [Arc::new(VmSpace::new()), Arc::new(VmSpace::new())];
    let c = [Arc::new(VmSpace::new()), Arc::new(VmSpace::new())];

    ostd::timer::register_callback_on_cpu(move || {
        let n = CALLBACK_RUNS.fetch_add(1, Ordering::Relaxed);
        c[n % 2].activate();
    });

    for i in 0..ROUNDS {
        t[i % 2].activate();
    }
    ostd::miri_println!(
        "vmspace-activate-irq: {} rounds done, timer callback ran {} times",
        ROUNDS,
        CALLBACK_RUNS.load(Ordering::Relaxed)
    );
}
