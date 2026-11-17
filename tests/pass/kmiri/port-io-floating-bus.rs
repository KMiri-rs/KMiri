//! `kern_miri_port_read` and `kern_miri_port_write` are x86 port I/O on
//! KMiri's port bus, used by a kernel's arch layer (e.g. OSTD's `IoPort`).
//!
//! No device is modelled behind any port yet, so the bus floats: a read
//! returns all ones for the access width and a write is dropped. This is what
//! a PC returns for a port that nothing decodes, so drivers see "no device".
extern "Rust" {
    fn kern_miri_port_read(port: u16, size: usize) -> u32;
    fn kern_miri_port_write(port: u16, size: usize, value: u32);
}

fn main() {
    unsafe {
        assert_eq!(kern_miri_port_read(0x80, 1), 0xff);
        assert_eq!(kern_miri_port_read(0x1f0, 2), 0xffff);
        assert_eq!(kern_miri_port_read(0xcfc, 4), 0xffff_ffff);

        // Writes are dropped: reading back still sees the floating bus.
        kern_miri_port_write(0x80, 1, 0x12);
        kern_miri_port_write(0xcf8, 4, 0x8000_0000);
        assert_eq!(kern_miri_port_read(0x80, 1), 0xff);
        assert_eq!(kern_miri_port_read(0xcfc, 4), 0xffff_ffff);
    }
}
