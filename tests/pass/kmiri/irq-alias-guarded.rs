// Masking IRQs protects the mutable reference while timer events accumulate.
// No IRQ API or yielding call occurs inside the loop; ticks still arise at block boundaries.
// Expect no UB and at least one handler run after dropping the guard, not an exact count.
//@compile-flags: -Zkmiri-toml=dev/test_utils/irq-timer.toml

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;

static mut STATE: u32 = 0;

extern "Rust" fn handler() {
    unsafe {
        *core::ptr::addr_of_mut!(STATE) += 1;
    }
}

fn main() {
    irq::register_timer_irq(handler);
    {
        let _guard = irq::IrqGuard::new();
        let r = unsafe { &mut *core::ptr::addr_of_mut!(STATE) };
        for i in 0..1000 {
            *r = i;
        }
    }
    // The loop leaves 999; queued handlers increment it after the reference's last use.
    assert!(unsafe { *core::ptr::addr_of_mut!(STATE) } >= 1000);
}
