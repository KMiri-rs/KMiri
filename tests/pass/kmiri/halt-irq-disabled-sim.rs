//! When IRQ simulation is disabled, halt returns immediately and prints a note once.

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;

fn main() {
    irq::halt();
    irq::halt();
}
