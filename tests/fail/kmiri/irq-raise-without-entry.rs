// Explicit raise must abort immediately when no entry has been registered.
// Use a valid vector so the error specifically checks the missing entry.
//@error-in-other-file: kern_miri_test_raise_irq: vector 40 raised before IRQ entry was registered
//@compile-flags: -Zkmiri-toml=dev/test_utils/irq.toml

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;

const TIMER_VECTOR: u8 = 40;

fn main() {
    irq::raise(TIMER_VECTOR);
}
