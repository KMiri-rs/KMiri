//! Control for `tests/vmspace-activate-irq`: the same program, but the main
//! loop disables local IRQs around each `activate`, so the timer callback
//! cannot run inside it. It uses that test's `kmiri.toml`, so timer interrupts
//! are injected at the same rate.
//!
//! Expected: no Undefined Behavior; the timer callback runs and the kernel
//! exits normally.
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
        let _irq = ostd::irq::disable_local();
        t[i % 2].activate();
    }
    let runs = CALLBACK_RUNS.load(Ordering::Relaxed);
    ostd::miri_println!(
        "vmspace-activate-irq-control: {} rounds done, timer callback ran {} times",
        ROUNDS,
        runs
    );
    assert!(runs > 0, "no timer interrupt was delivered");
}
