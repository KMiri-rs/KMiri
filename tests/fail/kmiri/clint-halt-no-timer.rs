//@compile-flags: -Zkmiri-toml=dev/test_utils/clint.toml -Zmiri-permissive-provenance
//@error-in-other-file: all started CPUs are halted and no interrupt can wake them
//! With no timer interrupt set and no software interrupt sent, nothing can wake
//! a halted CPU: the run stops instead of waiting forever.

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;

extern "Rust" fn handler(_vector: u8) {}

fn main() {
    irq::register_entry(handler);
    irq::halt();
}
