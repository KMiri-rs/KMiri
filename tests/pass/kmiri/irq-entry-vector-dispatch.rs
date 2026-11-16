// Queue two distinct vectors while masked, then verify FIFO dispatch through one entry.
// Explicit raises make the ordering deterministic; no automatic timer is configured.
// Both vectors must arrive unchanged and in order after the guard is dropped.
//@compile-flags: -Zkmiri-toml=dev/test_utils/irq.toml

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;

const TIMER_VECTOR: u8 = 40;

static mut RECEIVED: [u8; 2] = [0; 2];
static mut COUNT: usize = 0;

extern "Rust" fn entry(vector: u8) {
    unsafe {
        let count = *core::ptr::addr_of_mut!(COUNT);
        (*core::ptr::addr_of_mut!(RECEIVED))[count] = vector;
        *core::ptr::addr_of_mut!(COUNT) = count + 1;
    }
}

fn main() {
    irq::register_entry(entry);
    {
        let _guard = irq::IrqGuard::new();
        irq::raise(TIMER_VECTOR);
        irq::raise(TIMER_VECTOR + 1);
    }
    assert_eq!(unsafe { *core::ptr::addr_of_mut!(COUNT) }, 2);
    assert_eq!(unsafe { *core::ptr::addr_of_mut!(RECEIVED) }, [40, 41]);
}
