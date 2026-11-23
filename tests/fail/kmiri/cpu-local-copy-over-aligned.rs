//@compile-flags: -Zkmiri-toml=dev/test_utils/cpu-local-128k.toml -Zmiri-permissive-provenance
//! A larger `cpu_local_size` lets a variable aligned above the page size be placed.
//! In a copy of the section at a page boundary that is not aligned for it, a
//! reference to the variable's copy is unaligned, which KMiri must report.

#[path = "../../../dev/test_utils/smp.rs"]
mod smp;
#[path = "../../../dev/test_utils/physical_copy.rs"]
mod utils;

use std::sync::atomic::AtomicU64;

#[repr(align(0x10000))]
struct Big(#[allow(dead_code)] AtomicU64);

#[unsafe(link_section = ".cpu_local")]
static BIG: Big = Big(AtomicU64::new(7));
/// Page-aligned, but not aligned to 64 KiB.
const COPY_PADDR: usize = 0x0200_1000;
const COPY: usize = utils::LINEAR + COPY_PADDR;

fn main() {
    let _ = smp::local(0, &BIG);
    unsafe { utils::prepare_paging() };
    smp::copy_section(COPY_PADDR, COPY);
    let _big: &Big = unsafe { &*smp::local(COPY, &BIG) }; //~ ERROR: unaligned reference
}
