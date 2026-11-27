//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2-irq.toml
//! What returning from a handler does to the interrupt flag depends on the
//! architecture. x86 `iret` restores the flags saved on entry. RISC-V `mret`
//! and Arm `eret` take it from a per-CPU register that the kernel's own trap
//! return code sets, so the flag stays as the program left it.

#[path = "../../../dev/test_utils/irq.rs"]
mod irq;

use std::sync::atomic::{AtomicBool, Ordering};

unsafe extern "Rust" {
    fn kern_miri_irq_is_enabled() -> bool;
}

static RAN: AtomicBool = AtomicBool::new(false);

extern "Rust" fn handler(_vector: u8) {
    // Interrupts are off inside the handler, and it leaves them off.
    assert!(!unsafe { kern_miri_irq_is_enabled() });
    RAN.store(true, Ordering::SeqCst);
}

fn main() {
    irq::register_entry(handler);
    irq::enable();
    irq::raise(40);
    std::hint::black_box(());
    assert!(RAN.load(Ordering::SeqCst));
    let enabled = unsafe { kern_miri_irq_is_enabled() };
    if cfg!(any(target_arch = "x86", target_arch = "x86_64")) {
        assert!(enabled);
    } else {
        assert!(!enabled);
    }
}
