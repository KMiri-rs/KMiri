//! Shared fixture for the `virtio-blk` tests: a minimal driver for the virtio
//! block device of `virtio-blk.toml` (virtio-mmio version 2, one queue of 4
//! entries), on top of the `physical_copy` fixture.
//!
//! The rings and the request header and status byte are on untyped pages at
//! `RINGS`; each test chooses where the data buffer is.

#![allow(dead_code)]

#[path = "physical_copy.rs"]
mod physical_copy;

pub use physical_copy::*;

unsafe extern "Rust" {
    pub fn kern_miri_mmio_read(ptr: *const u8, size: usize) -> u64;
    pub fn kern_miri_mmio_write(ptr: *mut u8, size: usize, value: u64);
}

/// The physical address of the device's registers, as in `virtio-blk.toml`.
pub const DEVICE: usize = 0xfeb0_0000;
pub const RINGS: usize = SRC;
pub const DESC: usize = RINGS;
pub const AVAIL: usize = RINGS + 0x100;
pub const USED: usize = RINGS + 0x200;
pub const HEADER: usize = RINGS + 0x300;
pub const STATUS: usize = RINGS + 0x310;
pub const QUEUE_SIZE: u64 = 4;

pub const T_IN: u32 = 0;
pub const T_OUT: u32 = 1;

pub unsafe fn reg_read(offset: usize) -> u32 {
    unsafe { kern_miri_mmio_read(at(LINEAR + DEVICE + offset), 4) as u32 }
}

pub unsafe fn reg_write(offset: usize, value: u32) {
    unsafe { kern_miri_mmio_write(at(LINEAR + DEVICE + offset), 4, value.into()) }
}

/// Physical memory at `paddr` as a pointer to `T`, through `LINEAR`.
pub fn phys<T>(paddr: usize) -> *mut T {
    at(LINEAR + paddr).cast()
}

/// Sets up the paging, the ring pages, and the device with queue 0 ready,
/// following the driver initialization of VIRTIO 1.3, 3.1.1.
pub unsafe fn init_device() {
    unsafe {
        prepare_paging();
        kern_miri_alloc_pages(RINGS, 1);
        kern_miri_zero(RINGS, 1);

        assert_eq!(reg_read(0x000), 0x7472_6976);
        assert_eq!(reg_read(0x004), 2);
        assert_eq!(reg_read(0x008), 2);
        reg_write(0x070, 0);
        reg_write(0x070, 1 | 2);
        reg_write(0x014, 1);
        assert_eq!(reg_read(0x010), 1, "VIRTIO_F_VERSION_1");
        reg_write(0x024, 1);
        reg_write(0x020, 1);
        reg_write(0x070, 1 | 2 | 8);
        assert_eq!(reg_read(0x070), 1 | 2 | 8);
        reg_write(0x030, 0);
        assert!(reg_read(0x034) >= QUEUE_SIZE as u32);
        reg_write(0x038, QUEUE_SIZE as u32);
        reg_write(0x080, DESC as u32);
        reg_write(0x084, 0);
        reg_write(0x090, AVAIL as u32);
        reg_write(0x094, 0);
        reg_write(0x0a0, USED as u32);
        reg_write(0x0a4, 0);
        reg_write(0x044, 1);
        reg_write(0x070, 1 | 2 | 8 | 4);
    }
}

unsafe fn set_desc(index: usize, addr: usize, len: u32, flags: u16, next: u16) {
    unsafe {
        let desc = DESC + 16 * index;
        phys::<u64>(desc).write_volatile(addr as u64);
        phys::<u32>(desc + 8).write_volatile(len);
        phys::<u16>(desc + 12).write_volatile(flags);
        phys::<u16>(desc + 14).write_volatile(next);
    }
}

/// Makes a request of type `typ` for `sector` with a 512-byte data buffer at
/// physical address `data` available to the device, without notifying it, and
/// returns its index in the available ring.
pub unsafe fn submit(typ: u32, sector: u64, data: usize) -> u16 {
    unsafe {
        phys::<u32>(HEADER).write_volatile(typ);
        phys::<u32>(HEADER + 4).write_volatile(0);
        phys::<u64>(HEADER + 8).write_volatile(sector);
        phys::<u8>(STATUS).write_volatile(0xff);
        let data_flags = if typ == T_IN { 2 } else { 0 };
        set_desc(0, HEADER, 16, 1, 1);
        set_desc(1, data, 512, data_flags | 1, 2);
        set_desc(2, STATUS, 1, 2, 0);

        let idx = phys::<u16>(AVAIL + 2).read_volatile();
        phys::<u16>(AVAIL + 4 + 2 * (idx as usize % QUEUE_SIZE as usize)).write_volatile(0);
        phys::<u16>(AVAIL + 2).write_volatile(idx.wrapping_add(1));
        idx
    }
}

/// The MMIO write that notifies queue 0 (`QueueNotify`).
pub unsafe fn notify() {
    unsafe { kern_miri_mmio_write(at(LINEAR + DEVICE + 0x050), 4, 0) }
}

/// Makes a request (see `submit`), notifies the device, and returns the status
/// byte once the request is in the used ring.
pub unsafe fn request(typ: u32, sector: u64, data: usize) -> u8 {
    unsafe {
        let idx = submit(typ, sector, data);
        notify();
        assert_eq!(phys::<u16>(USED + 2).read_volatile(), idx.wrapping_add(1));
        let elem = USED + 4 + 8 * (idx as usize % QUEUE_SIZE as usize);
        assert_eq!(phys::<u32>(elem).read_volatile(), 0, "the chain's head");
        let status = phys::<u8>(STATUS).read_volatile();
        let written = phys::<u32>(elem + 4).read_volatile();
        assert_eq!(written, if typ == T_IN && status == 0 { 513 } else { 1 });
        status
    }
}

/// The initial content of byte `i` of the disk.
pub fn disk_byte(i: usize) -> u8 {
    (i % 251) as u8
}
