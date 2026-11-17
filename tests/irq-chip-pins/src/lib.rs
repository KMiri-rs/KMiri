//! The KMiri arch must provide an IRQ chip, as the x86 arch does.
//!
//! Drivers in the full Asterinas kernel (i8042, serial, VirtIO-MMIO) map their
//! IRQ pins through `ostd::arch::irq::IRQ_CHIP`. The KMiri arch had no IRQ
//! chip, so those drivers did not compile and were cut out with `cfg(miri)`.
//! The chip now models one I/O APIC with 24 pins (GSIs 0 to 23) and reports
//! the same errors as the x86 one.
//!
//! Expected: the checks pass and the kernel exits normally.
#![no_std]
#![deny(unsafe_code)]
#![feature(format_args_nl)]

extern crate alloc;

use ostd::{Error, arch::irq::IRQ_CHIP, irq::IrqLine};

#[ostd::ktest::miri_main]
fn kernel_main() {
    let chip = IRQ_CHIP.get().unwrap();
    assert_eq!(chip.count_io_apics(), 1);

    // ISA IRQ 4 (COM1) maps to GSI 4.
    let com1 = chip.map_isa_pin_to(IrqLine::alloc().unwrap(), 4).unwrap();

    // A pin is mapped at most once.
    let busy = chip.map_gsi_pin_to(IrqLine::alloc().unwrap(), 4);
    assert!(matches!(busy, Err(Error::AccessDenied)));

    // GSI 24 does not exist.
    let missing = chip.map_gsi_pin_to(IrqLine::alloc().unwrap(), 24);
    assert!(matches!(missing, Err(Error::InvalidArgs)));

    // Dropping the mapping frees the pin.
    drop(com1);
    assert!(chip.map_gsi_pin_to(IrqLine::alloc().unwrap(), 4).is_ok());

    ostd::miri_println!("irq-chip-pins: ok");
}
