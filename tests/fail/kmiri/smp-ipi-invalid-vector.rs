//@error-in-other-file: vector 31 is reserved for CPU exceptions
//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2-irq.toml
//! Sending an IPI with a reserved exception vector (< 32) aborts.

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;

extern "Rust" fn handler(_vector: u8) {}

fn main() {
    irq::register_entry(handler);
    irq::send_ipi(0, 31);
}
