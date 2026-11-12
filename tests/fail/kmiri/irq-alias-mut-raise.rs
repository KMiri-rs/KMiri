// Explicit delivery deterministically invalidates the task's mutable reference.
// Unlike the timer loops, this case intentionally calls the IRQ API to fix the timing.
// raise_irq delivers before returning, so the final write must fail Stacked Borrows.
//@compile-flags: -Zkmiri-toml=dev/test_utils/irq.toml

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;

static mut STATE: u32 = 0;

extern "Rust" fn handler() {
    unsafe {
        *core::ptr::addr_of_mut!(STATE) += 1;
    }
}

fn main() {
    let r = unsafe { &mut *core::ptr::addr_of_mut!(STATE) };
    *r = 1;
    irq::raise_irq(handler);
    *r = 2; //~ ERROR: attempting a write access
}
