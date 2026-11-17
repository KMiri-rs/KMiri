//! Port I/O and the I/O allocators must work under KMiri.
//!
//! Drivers in the full Asterinas kernel probe their hardware through `IoPort`
//! and `IoMem`. The KMiri arch layer used to leave both allocators
//! uninitialized and port accesses `unimplemented!()`. Port I/O now goes to
//! KMiri's port bus, where no device is modelled yet: a read returns all ones
//! and a write is dropped, as for a PC port that nothing decodes. The machine
//! has no MMIO range, so `IoMem::acquire` fails.
//!
//! Expected: the checks pass and the kernel exits normally.
#![no_std]
#![deny(unsafe_code)]
#![feature(format_args_nl)]

use ostd::{
    arch::device::io_port::ReadWriteAccess,
    io::{IoMem, IoPort},
};

#[ostd::ktest::miri_main]
fn kernel_main() {
    // PCI configuration mechanism #1: no device answers, so the data port
    // reads all ones (vendor ID 0xffff means "no device").
    let pci_addr = IoPort::<u32, ReadWriteAccess>::acquire(0xcf8).unwrap();
    let pci_data = IoPort::<u32, ReadWriteAccess>::acquire(0xcfc).unwrap();
    pci_addr.write(0x8000_0000);
    assert_eq!(pci_data.read(), 0xffff_ffff);

    let byte = IoPort::<u8, ReadWriteAccess>::acquire(0x80).unwrap();
    byte.write(0x12);
    assert_eq!(byte.read(), 0xff);
    let word = IoPort::<u16, ReadWriteAccess>::acquire(0x1f0).unwrap();
    assert_eq!(word.read(), 0xffff);

    // The allocator hands each port out once.
    assert!(IoPort::<u8, ReadWriteAccess>::acquire(0x80).is_err());
    drop(byte);
    assert!(IoPort::<u8, ReadWriteAccess>::acquire(0x80).is_ok());

    // The machine has no MMIO range.
    assert!(IoMem::acquire(0xfeb0_0000..0xfeb0_1000).is_err());

    ostd::miri_println!("port-io-bus: ok");
}
