// Registering even the same entry twice must abort, not silently replace it.
// No IRQ is raised; this checks the registration contract itself.
//@error-in-other-file: kern_miri_register_irq_entry: IRQ entry already registered
//@compile-flags: -Zkmiri-toml=dev/test_utils/irq.toml

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;

const TIMER_VECTOR: u8 = 40;

extern "Rust" fn entry(vector: u8) {
    assert_eq!(vector, TIMER_VECTOR);
}

fn main() {
    irq::register_entry(entry);
    irq::register_entry(entry);
}
