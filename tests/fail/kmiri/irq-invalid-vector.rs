// Reject reserved CPU-exception vectors instead of queuing an external IRQ.
// Register first to isolate vector validation from the missing-entry check.
//@error-in-other-file: kern_miri_test_raise_irq: vector 3 is reserved for CPU exceptions
//@compile-flags: -Zkmiri-toml=dev/test_utils/irq.toml

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;

const TIMER_VECTOR: u8 = 40;

extern "Rust" fn entry(vector: u8) {
    assert_eq!(vector, TIMER_VECTOR);
}

fn main() {
    irq::register_entry(entry);
    irq::raise(3);
}
