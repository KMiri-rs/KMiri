//@compile-flags: -Zkmiri-toml=dev/test_utils/pool.toml -Zmiri-permissive-provenance
//! A thread whose stack was freed has ended; switching back to it is reported.

unsafe extern "Rust" {
    fn kern_miri_object_born(ptr: *mut u8, size: usize, align: usize) -> *mut u8;
    fn kern_miri_object_died(ptr: *mut u8) -> *mut u8;
}

const POOL: usize = 0x2000000;

/// The block at `POOL + offset`, reached through its address as an allocator does.
fn block(offset: usize) -> *mut u8 {
    std::ptr::with_exposed_provenance_mut(POOL + offset)
}

unsafe extern "Rust" {
    fn miri_create_new_thread(
        func: fn(usize),
        arg: usize,
        stack_end: usize,
        stack_size: usize,
    ) -> usize;
    fn miri_switch_to(thread: usize);
}

use std::sync::atomic::{AtomicUsize, Ordering};

static VICTIM: AtomicUsize = AtomicUsize::new(0);
static OTHER: AtomicUsize = AtomicUsize::new(0);
static STACK_PTR: AtomicUsize = AtomicUsize::new(0);

fn victim(_: usize) {
    unsafe { miri_switch_to(OTHER.load(Ordering::SeqCst)) };
}

fn other(_: usize) {
    unsafe {
        kern_miri_object_died(std::ptr::with_exposed_provenance_mut(
            STACK_PTR.load(Ordering::SeqCst),
        ));
        miri_switch_to(VICTIM.load(Ordering::SeqCst)); //~ ERROR: stack was freed
    }
}

fn main() {
    unsafe {
        let stack = kern_miri_object_born(block(0), 0x10000, 16);
        STACK_PTR.store(stack.expose_provenance(), Ordering::SeqCst);
        VICTIM.store(
            miri_create_new_thread(victim, 0, stack.addr() + 0x10000, 0x10000),
            Ordering::SeqCst,
        );
        OTHER.store(miri_create_new_thread(other, 0, 0x6000000, 0x100000), Ordering::SeqCst);
        miri_switch_to(VICTIM.load(Ordering::SeqCst));
    }
}
