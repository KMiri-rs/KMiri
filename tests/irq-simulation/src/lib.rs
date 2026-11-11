#![no_std]
#![deny(unsafe_code)]
#![feature(format_args_nl)]

extern crate alloc;

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

static TIMER_CALLBACKS: AtomicUsize = AtomicUsize::new(0);
static CALLBACK_INTERRUPT_LEVEL: AtomicUsize = AtomicUsize::new(0);
static CALLBACK_IRQS_DISABLED: AtomicBool = AtomicBool::new(false);

#[ostd::ktest::miri_main]
fn kernel_main() {
    assert!(ostd::irq::miri_local_irqs_enabled());

    ostd::timer::register_callback_on_cpu(|| {
        CALLBACK_INTERRUPT_LEVEL.store(
            if ostd::irq::InterruptLevel::current()
                == ostd::irq::InterruptLevel::L1(ostd::cpu::PrivilegeLevel::Kernel)
            {
                1
            } else {
                0
            },
            Ordering::SeqCst,
        );
        CALLBACK_IRQS_DISABLED.store(!ostd::irq::miri_local_irqs_enabled(), Ordering::SeqCst);
        TIMER_CALLBACKS.fetch_add(1, Ordering::SeqCst);
    });

    // The nested guard must preserve the outer guard's disabled state. Raising
    // the simulated timer IRQ queues its real OSTD timer entry, whose callback
    // must wait until the outer guard restores local IRQs.
    let outer_irq = ostd::irq::disable_local();
    assert!(!ostd::irq::miri_local_irqs_enabled());
    let inner_irq = ostd::irq::disable_local();
    assert!(!ostd::irq::miri_local_irqs_enabled());
    ostd::timer::miri_test_raise_timer_irq();
    assert!(!ostd::irq::miri_local_irqs_enabled());
    assert_eq!(TIMER_CALLBACKS.load(Ordering::SeqCst), 0);

    drop(inner_irq);
    assert!(!ostd::irq::miri_local_irqs_enabled());
    assert_eq!(TIMER_CALLBACKS.load(Ordering::SeqCst), 0);

    drop(outer_irq);
    assert!(ostd::irq::miri_local_irqs_enabled());
    assert_eq!(TIMER_CALLBACKS.load(Ordering::SeqCst), 1);
    assert_eq!(CALLBACK_INTERRUPT_LEVEL.load(Ordering::SeqCst), 1);
    assert!(CALLBACK_IRQS_DISABLED.load(Ordering::SeqCst));

    ostd::miri_println!("irq-simulation: pending timer delivered at L1 kernel context");
}
