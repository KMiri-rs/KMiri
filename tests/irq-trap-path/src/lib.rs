// SPDX-License-Identifier: MPL-2.0

//! Checks that a KMiri timer IRQ goes through Asterinas' native interrupt path
//! (`trap_handler` -> top half -> bottom half), not a shortcut into the timer
//! callbacks.
//!
//! The bottom half only runs when the IRQ is dispatched through
//! `call_irq_callback_functions`. A shortcut that calls the timer callbacks
//! directly never reaches it, so `BOTTOM_HALF_RUNS` would stay 0 and the
//! final assertion would fail.

#![no_std]
#![deny(unsafe_code)]
#![feature(format_args_nl)]

extern crate alloc;

use core::sync::atomic::{AtomicUsize, Ordering};

static TICKS: AtomicUsize = AtomicUsize::new(0);
static BOTTOM_HALF_RUNS: AtomicUsize = AtomicUsize::new(0);

fn bottom_half_l1(
    guard: ostd::irq::DisabledLocalIrqGuard,
    _irq_num: u8,
) -> ostd::irq::DisabledLocalIrqGuard {
    BOTTOM_HALF_RUNS.fetch_add(1, Ordering::SeqCst);
    guard
}

#[ostd::ktest::miri_main]
fn kernel_main() {
    ostd::irq::register_bottom_half_handler_l1(bottom_half_l1);
    ostd::timer::register_callback_on_cpu(|| {
        TICKS.fetch_add(1, Ordering::SeqCst);
    });

    // IRQs are enabled here, so KMiri delivers the raised timer IRQ at the
    // next basic-block boundary, before the loads below.
    assert!(ostd::irq::miri_local_irqs_enabled());
    ostd::timer::miri_test_raise_timer_irq();

    let ticks = TICKS.load(Ordering::SeqCst);
    let bottom_half_runs = BOTTOM_HALF_RUNS.load(Ordering::SeqCst);
    ostd::miri_println!(
        "irq-trap-path: ticks={} bottom_half_runs={}",
        ticks,
        bottom_half_runs
    );
    assert_eq!(ticks, 1, "the raised timer IRQ was not delivered");
    assert_eq!(
        bottom_half_runs, 1,
        "the timer IRQ bypassed trap_handler: the bottom half never ran"
    );
    ostd::miri_println!("irq-trap-path: native trap path verified");
}
