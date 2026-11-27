//@error-in-other-file: all started CPUs are halted and no interrupt can wake them
//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2-irq.toml
//! All started CPUs halted with no interrupt source that can wake them.

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;
#[path = "../../../dev/test_utils/smp.rs"]
mod smp;

fn cpu1_entry(_: usize) {
    irq::halt();
}

fn main() {
    smp::start(1, cpu1_entry, 0);
    irq::halt();
}
