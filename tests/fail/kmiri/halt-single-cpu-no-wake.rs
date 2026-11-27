//@error-in-other-file: all started CPUs are halted and no interrupt can wake them
//@compile-flags: -Zkmiri-toml=dev/test_utils/irq.toml
//! A single CPU halted with no interrupt source that can wake it.

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;

fn main() {
    irq::halt();
}
