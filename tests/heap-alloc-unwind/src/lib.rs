//! Asterinas issue 3910: a safe global heap allocator can unwind out of OSTD's
//! `GlobalAlloc` implementation.
//!
//! The kernel registers its own heap allocator with OSTD's public macros. It
//! forwards to the default OSDK allocator, but when `UNWIND_NEXT` is set it
//! first calls `ostd::panic::begin_panic`. `kernel_main` sets the flag inside
//! `ostd::panic::catch_unwind` and allocates a `Box`. The unwind starts in the
//! allocator, passes through OSTD's `AllocDispatch::alloc`, and leaves through
//! the `__rust_alloc` call, which Rust declares cannot unwind.
//!
//! Expected: Undefined Behavior "unwinding past a stack frame that does not
//! allow unwinding".
#![no_std]
#![deny(unsafe_code)]
#![feature(allocator_api)]
#![cfg_attr(miri, feature(format_args_nl))]

extern crate alloc;

use alloc::boxed::Box;
use core::{
    alloc::{AllocError, Layout},
    sync::atomic::{AtomicBool, Ordering},
};

use osdk_heap_allocator::{HeapAllocator, type_from_layout};
use ostd::mm::heap::{GlobalHeapAllocator, HeapSlot, SlotInfo};

// Diagnostics only; compiled out outside Miri.
macro_rules! note {
    ($($t:tt)*) => { #[cfg(miri)] ostd::miri_println!($($t)*); };
}

static UNWIND_NEXT: AtomicBool = AtomicBool::new(false);

struct UnwindingHeap(HeapAllocator);

impl GlobalHeapAllocator for UnwindingHeap {
    fn alloc(&self, layout: Layout) -> Result<HeapSlot, AllocError> {
        if UNWIND_NEXT.swap(false, Ordering::Relaxed) {
            // The flag is already cleared, so allocating the payload here does not recurse.
            let _ = ostd::panic::begin_panic(Box::new(7u32));
        }
        self.0.alloc(layout)
    }

    fn dealloc(&self, slot: HeapSlot) -> Result<(), AllocError> {
        self.0.dealloc(slot)
    }
}

#[ostd::global_heap_allocator]
static HEAP: UnwindingHeap = UnwindingHeap(HeapAllocator);

#[ostd::global_heap_allocator_slot_map]
const fn slot_map(layout: Layout) -> Option<SlotInfo> {
    type_from_layout(layout)
}

#[cfg_attr(miri, ostd::ktest::miri_main)]
#[cfg_attr(not(miri), ostd::main)]
fn kernel_main() {
    let result = ostd::panic::catch_unwind(|| {
        UNWIND_NEXT.store(true, Ordering::Relaxed);
        Box::new(5u64)
    });
    match result {
        Ok(b) => {
            note!("heap-alloc-unwind: allocation returned {}", b);
        }
        Err(_) => {
            note!("heap-alloc-unwind: unwind caught");
        }
    }
}
