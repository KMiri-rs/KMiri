// Fresh raw accesses do not keep a mutable-reference tag across an IRQ.
// The loop has no IRQ API or yielding call, so timer injection remains asynchronous.
// Expect no aliasing UB; this is not an atomicity or lost-update guarantee.
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
    for _ in 0..1000 {
        unsafe {
            *core::ptr::addr_of_mut!(STATE) = *core::ptr::addr_of_mut!(STATE) + 1;
        }
    }
}
