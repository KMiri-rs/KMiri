//@error-in-other-file: halt inside an interrupt handler is not supported
//@compile-flags: -Zkmiri-toml=dev/test_utils/irq.toml
//! Halting inside an interrupt handler is not supported.

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;

extern "Rust" fn handler(_vector: u8) {
    irq::halt();
}

fn main() {
    irq::register_entry(handler);
    irq::enable();
    irq::raise(40);
}
