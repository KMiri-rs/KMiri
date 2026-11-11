#![no_std]
#![deny(unsafe_code)]
#![feature(format_args_nl, stmt_expr_attributes)]

extern crate alloc;

use ostd::sync::{RwLock, WriteIrqDisabled};

static LOCK: RwLock<(), WriteIrqDisabled> = RwLock::new(());

#[ostd::ktest::miri_main]
fn kernel_main() {
    ostd::timer::register_callback_on_cpu(|| {
        // The interrupted context holds a read lock. The timer callback runs
        // on the same CPU and spins forever while trying to acquire the write
        // lock. KMiri's IRQ watchdog must report this as a possible deadlock.
        let _write_guard = LOCK.write();
    });

    let _read_guard = LOCK.read();
    assert!(ostd::irq::miri_local_irqs_enabled());
    ostd::miri_println!("irq-deadlock: raising timer IRQ while holding read lock");
    ostd::timer::miri_test_raise_timer_irq();

    panic!("timer IRQ handler unexpectedly returned while read lock is held");
}
