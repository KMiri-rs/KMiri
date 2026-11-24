//@compile-flags: -Zkmiri-toml=dev/test_utils/virtio-blk.toml -Zmiri-permissive-provenance

//! The virtio block device that `[virtio_blk]` in the KMiri config adds: a
//! driver negotiates, sets up a queue, and reads and writes sectors. The
//! device reads the rings and writes the data and the status byte by DMA.
//!
//! - A read into untyped pages fills them with the disk's bytes.
//! - A write from untyped pages, read back into a mutable static, returns
//!   those bytes: the device writes an interpreter allocation like any
//!   write through a pointer cast from its address. The static's address was
//!   never exposed, and a pointer to it taken before still reads it.
//! - A request past the end of the disk fails with an I/O error and writes
//!   no data.
//! - Away from the device's registers, the MMIO bus still floats.
//! - After a reset, features without `VIRTIO_F_VERSION_1` are refused:
//!   `FEATURES_OK` does not stick.

#[path = "../../../dev/test_utils/virtio_blk.rs"]
mod utils;

use utils::*;

static mut BUF: [u8; 512] = [0; 512];

fn main() {
    unsafe {
        init_device();
        assert_eq!(kern_miri_mmio_read(at(LINEAR + DEVICE + 0x100), 8), 8, "capacity");

        let data = DST;
        kern_miri_alloc_pages(data, 1);
        assert_eq!(request(T_IN, 1, data), 0);
        for i in 0..512 {
            assert_eq!(phys::<u8>(data + i).read_volatile(), disk_byte(512 + i));
        }
        assert_eq!(reg_read(0x060), 1, "used buffer notification");
        reg_write(0x064, 1);
        assert_eq!(reg_read(0x060), 0);

        for i in 0..512 {
            phys::<u8>(data + i).write_volatile(i as u8 ^ 0x5a);
        }
        assert_eq!(request(T_OUT, 3, data), 0);
        let before = (&raw const BUF).cast::<u8>();
        let buf = (&raw mut BUF).addr() - KERNEL;
        assert_eq!(request(T_IN, 3, buf), 0);
        for i in 0..512 {
            assert_eq!(before.add(i).read(), i as u8 ^ 0x5a);
        }

        assert_eq!(request(T_IN, 8, data), 1, "VIRTIO_BLK_S_IOERR");
        assert_eq!(phys::<u8>(data).read_volatile(), 0x5a, "no data written");

        assert_eq!(kern_miri_mmio_read(at(LINEAR + DEVICE + 0x200), 4), 0xffff_ffff);

        reg_write(0x070, 0);
        reg_write(0x070, 1 | 2);
        reg_write(0x024, 1);
        reg_write(0x020, 0);
        reg_write(0x070, 1 | 2 | 8);
        assert_eq!(reg_read(0x070), 1 | 2, "FEATURES_OK refused");
    }
}
