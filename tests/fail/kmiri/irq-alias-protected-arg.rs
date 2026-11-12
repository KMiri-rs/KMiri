// The work argument's protector must reject the handler's conflicting access.
// No IRQ API or yielding call occurs in the loop: timer delivery is asynchronous.
// Expect Stacked Borrows UB in handler, with handler, work, and main on the stack.
//@compile-flags: -Zkmiri-toml=dev/test_utils/irq-timer.toml

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;

static mut STATE: u32 = 0;

extern "Rust" fn handler() {
    unsafe {
        *core::ptr::addr_of_mut!(STATE) += 1; //~ ERROR: protected
    }
}

fn work(r: &mut u32) {
    for i in 0..1000 {
        *r = i;
    }
}

fn main() {
    irq::register_timer_irq(handler);
    work(unsafe { &mut *core::ptr::addr_of_mut!(STATE) });
}
