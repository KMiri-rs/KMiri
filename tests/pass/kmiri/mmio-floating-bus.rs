//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance

//! `kern_miri_mmio_read` and `kern_miri_mmio_write` are one MMIO load or store
//! at a kernel virtual address, used by a kernel's arch layer (e.g. OSTD's
//! `IoMem`).
//!
//! An address that maps to RAM is accessed like any other memory. One that
//! maps outside RAM (RAM is 128 MiB here) has no device behind it, so the bus
//! floats: a read returns all ones for the access width and a write is
//! dropped, which is what drivers see natively when no device is present.

use std::ptr;

#[path = "../../../dev/test_utils/physical_copy.rs"]
mod utils;

use utils::*;

extern "Rust" {
    fn kern_miri_mmio_read(ptr: *const u8, size: usize) -> u64;
    fn kern_miri_mmio_write(ptr: *mut u8, size: usize, value: u64);
}

/// QEMU microvm's first virtio-mmio slot, above RAM.
const DEVICE: usize = 0xfeb0_0000;

fn main() {
    unsafe {
        prepare_paging();

        let dev = ptr::with_exposed_provenance_mut::<u8>(LINEAR + DEVICE);
        assert_eq!(kern_miri_mmio_read(dev, 1), 0xff);
        assert_eq!(kern_miri_mmio_read(dev, 2), 0xffff);
        assert_eq!(kern_miri_mmio_read(dev, 4), 0xffff_ffff);
        assert_eq!(kern_miri_mmio_read(dev, 8), u64::MAX);
        // Writes are dropped: reading back still sees the floating bus.
        kern_miri_mmio_write(dev, 4, 0x1234_5678);
        assert_eq!(kern_miri_mmio_read(dev, 4), 0xffff_ffff);

        // RAM is ordinary memory.
        let mut ram: u32 = 0;
        let ram = (&raw mut ram).cast::<u8>();
        kern_miri_mmio_write(ram, 4, 0x1234_5678);
        assert_eq!(kern_miri_mmio_read(ram, 4), 0x1234_5678);
        assert_eq!(kern_miri_mmio_read(ram, 1), 0x78);
    }
}
