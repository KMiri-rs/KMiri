//! Shared fixture for the `physical-copy` tests: the `kern_miri_*` shims, the
//! test memory-layout constants, and a `prepare_paging` helper that maps a huge
//! region of virtual memory at `LINEAR` onto physical 0, letting each test
//! address physical memory directly as `LINEAR + X`.
//!
//! Test files pull this in with
//! `#[path = "../../dev/test_utils/physical_copy.rs"] mod utils; use utils::*;`.

#![allow(dead_code)]

use std::ptr;

unsafe extern "Rust" {
    pub fn kern_miri_alloc_pages(paddr: usize, count: usize);
    pub fn kern_miri_zero(paddr: usize, count: usize);
    pub fn kern_miri_write_bytes(paddr: usize, data: *const u8, len: usize);
    pub fn kern_miri_retype_pages(paddr: usize, count: usize, kind: TypedKind, slot_size: usize);
    pub fn kern_miri_copy_untyped(dst: usize, src: usize, len: usize);
    pub fn kern_miri_copy(dst: *mut u8, src: *const u8, len: usize);
    pub fn kern_miri_set_root_page_table(paddr: usize);
}

#[repr(usize)]
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TypedKind {
    Slab = 1,
    PageTable = 2,
    Stack = 3,
    Interpreter = 4,
}

// Test memory layout (physical addresses unless noted):
// - SRC / DST: the physical pages used as copy source and destination, placed
//   inside the huge `LINEAR` window so `LINEAR + X` reaches them.
// - LINEAR: the base of a canonical upper-half virtual region. `prepare_paging`
//   maps it onto physical 0, so `LINEAR + X` addresses physical `X` (the copy
//   source/destination are touched this way).
// - KERNEL: the kernel direct-map base: physical address `P` is accessed at
//   `KERNEL + P`. The test reads/writes the page table itself through it.
// - PT: the physical address of one page set aside for the page table.
// Not every test uses every constant (e.g. `overlap-*` only uses `SRC`), so
// silence the dead-code lint on the shared fixture.

pub const SRC: usize = 0x0400_0000;
pub const DST: usize = 0x0401_0000;
pub const LINEAR: usize = 0xffff_8000_0000_0000;
pub const KERNEL: usize = 0xffff_ffff_8000_0000;
pub const PT: usize = 0x0500_0000;

/// The virtual address `vaddr` as a pointer, the way a kernel passes an address
/// it computed (e.g. of a frame in its linear mapping) to `kern_miri_copy`.
pub fn at(vaddr: usize) -> *mut u8 {
    ptr::with_exposed_provenance_mut(vaddr)
}

// Build a minimal x86-64-style page table so that `LINEAR + X` resolves to
// physical `X`. A single present + huge (PS) PML4 entry for `LINEAR`'s index
// maps a 512 GiB region starting at `LINEAR` onto physical 0, covering the
// whole test physical memory.
//
// Steps:
// 1. `PT` receives one page, retyped as a page-table page (slot=8). Writing the
//    PML4 through the kernel direct map requires the page to be typed.
// 2. `root` is the PML4 entry for `LINEAR`'s PML4 index — (LINEAR >> 39) & 511
//    == 256 — i.e. byte 256 * 8 of the page, accessed via the direct map.
// 3. `root.write(0x81)` installs PML4[256] = present (bit 0) | huge-page (bit 7,
//    "PS") with physical base 0.
// 4. `kern_miri_set_root_page_table(PT)` makes it the active PML4.
//
// Net effect: `LINEAR + X` accesses physical `X`; SRC (64 MiB) and DST
// (64 MiB + 64 KiB) both lie inside that window.
pub unsafe fn prepare_paging() {
    kern_miri_alloc_pages(PT, 1);
    kern_miri_retype_pages(PT, 1, TypedKind::PageTable, 8);
    let root = ptr::with_exposed_provenance_mut::<usize>(KERNEL + PT + ((LINEAR >> 39) & 511) * 8);
    root.write(0x81);
    kern_miri_set_root_page_table(PT);
}
