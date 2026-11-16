//! A dynamically allocated CPU-local object must be usable under KMiri.
//!
//! `DynCpuLocalChunk` takes its frames directly from the frame allocator and
//! reaches them through `paddr_to_vaddr`, i.e. through an address computed
//! from an integer. Unless OSTD tells KMiri about these frames, the first
//! write to a CPU-local object fails with "dangling pointer (it has no
//! provenance)". The full Asterinas kernel hits this in `aster_softirq::init`,
//! at the first `PerCpuCounter::new`.
//!
//! The chunk tracks its free items with `bitvec`, which Stacked Borrows
//! rejects, so `kmiri.toml` turns on `sb_zero_size_retag_keeps_tag`.
//!
//! Expected: every step below completes and the kernel exits normally.
#![no_std]
#![deny(unsafe_code)]
#![feature(format_args_nl)]

extern crate alloc;

use core::sync::atomic::{AtomicU64, Ordering};

use ostd::cpu::{CpuId, local::DynCpuLocalChunk};

#[ostd::ktest::miri_main]
fn kernel_main() {
    let mut chunk = DynCpuLocalChunk::<8>::new().unwrap();
    let first = chunk.alloc(&mut |_| AtomicU64::new(1)).unwrap();
    let second = chunk.alloc(&mut |_| AtomicU64::new(2)).unwrap();
    ostd::miri_println!("dyn-cpu-local: alloc ok");

    let bsp = CpuId::bsp();
    first.get_on_cpu(bsp).fetch_add(10, Ordering::Relaxed);
    assert_eq!(first.get_on_cpu(bsp).load(Ordering::Relaxed), 11);
    assert_eq!(second.get_on_cpu(bsp).load(Ordering::Relaxed), 2);
    ostd::miri_println!("dyn-cpu-local: write and read ok");

    assert!(chunk.try_dealloc(first).is_ok());
    assert!(chunk.try_dealloc(second).is_ok());
    assert!(chunk.is_empty());
    ostd::miri_println!("dyn-cpu-local: dealloc ok");
}
