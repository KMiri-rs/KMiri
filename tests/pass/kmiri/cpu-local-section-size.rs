//@compile-flags: -Zkmiri-toml=dev/test_utils/cpu-local-128k.toml -Zmiri-permissive-provenance

//! `cpu_local_size` sets the size of the CPU-local section. The kernel code section
//! grows with it, so the kernel stack section after it keeps its default size.

use std::sync::atomic::{AtomicU64, Ordering};

/// Does not fit in the default 64 KiB section: its alignment alone needs 128 KiB.
#[repr(align(0x10000))]
struct Big(AtomicU64);

#[unsafe(link_section = ".cpu_local")]
static BIG: Big = Big(AtomicU64::new(7));

unsafe extern "C" {
    fn __cpu_local_start();
    fn __cpu_local_end();
    fn __kernel_start();
    fn __kernel_end();
}

fn main() {
    let start = __cpu_local_start as *const () as usize;
    let end = __cpu_local_end as *const () as usize;
    assert_eq!(end - start, 0x2_0000);

    let big = &BIG as *const Big as usize;
    assert!(big.is_multiple_of(0x1_0000));
    assert!(start <= big && big + size_of::<Big>() <= end);
    assert_eq!(BIG.0.load(Ordering::Relaxed), 7);

    // The default layout has a 16 MiB kernel code section with a 64 KiB CPU-local
    // section. The 64 KiB added here go to the kernel code section.
    let kernel_start = __kernel_start as *const () as usize;
    let kernel_end = __kernel_end as *const () as usize;
    assert_eq!(kernel_end - kernel_start, 0x100_0000 + 0x1_0000);
    assert_eq!(kernel_end - end, 0x100_0000 - 0x41_0000);

    let local = 0u8;
    let local = &raw const local as usize;
    assert!(end <= local && local < kernel_end);
}
