//! KMiri interrupt shims shared by the IRQ regression tests.

#![allow(dead_code)]

unsafe extern "Rust" {
    fn kern_miri_irq_is_enabled() -> bool;
    fn kern_miri_irq_disable();
    fn kern_miri_irq_enable();
    fn kern_miri_register_irq_entry(entry: extern "Rust" fn(u8));
    fn kern_miri_set_timer_vector(vector: u8);
    fn kern_miri_test_raise_irq(vector: u8);
}

pub struct IrqGuard {
    was_enabled: bool,
}

impl IrqGuard {
    pub fn new() -> Self {
        let was_enabled = unsafe { kern_miri_irq_is_enabled() };
        unsafe { kern_miri_irq_disable() };
        Self { was_enabled }
    }
}

impl Drop for IrqGuard {
    fn drop(&mut self) {
        if self.was_enabled {
            unsafe { kern_miri_irq_enable() };
        }
    }
}

pub fn raise(vector: u8) {
    unsafe { kern_miri_test_raise_irq(vector) };
}

pub fn register_entry(entry: extern "Rust" fn(u8)) {
    unsafe { kern_miri_register_irq_entry(entry) };
}

pub fn set_timer_vector(vector: u8) {
    unsafe { kern_miri_set_timer_vector(vector) };
}
