// An asynchronous IRQ invalidates the task's long-lived mutable reference.
// The loop uses no IRQ API or yielding call: only MIR block boundaries trigger ticks.
// A later loop write must fail Stacked Borrows after the handler accesses STATE.
//@compile-flags: -Zkmiri-toml=dev/test_utils/irq-timer.toml

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;

const TIMER_VECTOR: u8 = 40;

static mut STATE: u32 = 0;

extern "Rust" fn handler() {
    unsafe {
        *core::ptr::addr_of_mut!(STATE) += 1;
    }
}

extern "Rust" fn entry(vector: u8) {
    assert_eq!(vector, TIMER_VECTOR);
    handler();
}

fn main() {
    irq::register_entry(entry);
    irq::set_timer_vector(TIMER_VECTOR);
    let r = unsafe { &mut *core::ptr::addr_of_mut!(STATE) };
    for i in 0..1000 {
        *r = i; //~ ERROR: attempting a write access
    }
}
