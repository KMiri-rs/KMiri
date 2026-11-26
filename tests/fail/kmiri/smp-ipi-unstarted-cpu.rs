//@error-in-other-file: target CPU 1 has not been started
//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2-irq.toml
//! Sending an IPI to a CPU that has not been started aborts.

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;

extern "Rust" fn handler(_vector: u8) {}

fn main() {
    irq::register_entry(handler);
    irq::send_ipi(1, 40);
}
