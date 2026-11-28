//! Registers of the modelled CLINT at the address `clint.toml` gives.

#![allow(dead_code)]

use std::ptr::{read_volatile, write_volatile};

pub const BASE: usize = 0x200_0000;
pub const SOFTWARE: u8 = 3;
pub const TIMER: u8 = 7;

pub fn mtime() -> u64 {
    unsafe { read_volatile((BASE + 0xbff8) as *const u64) }
}

pub fn set_mtimecmp(cpu: usize, value: u64) {
    unsafe { write_volatile((BASE + 0x4000 + 8 * cpu) as *mut u64, value) }
}

pub fn mtimecmp(cpu: usize) -> u64 {
    unsafe { read_volatile((BASE + 0x4000 + 8 * cpu) as *const u64) }
}

pub fn set_msip(cpu: usize, value: u32) {
    unsafe { write_volatile((BASE + 4 * cpu) as *mut u32, value) }
}

pub fn msip(cpu: usize) -> u32 {
    unsafe { read_volatile((BASE + 4 * cpu) as *const u32) }
}
