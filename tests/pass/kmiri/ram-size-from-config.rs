//@compile-flags: -Zkmiri-toml=dev/test_utils/ram-size-64m.toml

//! `kern_miri_ram_size` reports the machine's RAM size from the KMiri config, which
//! a kernel's boot code turns into its memory map (RAM starts at physical address 0).
//!
//! Expected: the configured 64 MiB, not the 128 MiB default.

extern "Rust" {
    fn kern_miri_ram_size() -> usize;
}

fn main() {
    assert_eq!(unsafe { kern_miri_ram_size() }, 0x0400_0000);
}
