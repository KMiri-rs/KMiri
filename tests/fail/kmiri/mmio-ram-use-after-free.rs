//! The floating MMIO bus is only for addresses outside RAM. An MMIO access to
//! RAM is checked like any other access, so one through a dangling pointer is
//! still reported.

extern "Rust" {
    fn kern_miri_mmio_read(ptr: *const u8, size: usize) -> u64;
}

fn main() {
    let boxed = Box::new(0u32);
    let ptr = (&raw const *boxed).cast::<u8>();
    drop(boxed);
    unsafe { kern_miri_mmio_read(ptr, 4) }; //~ ERROR: has been freed
}
