//@compile-flags: -Zkmiri-toml=dev/test_utils/pool.toml -Zmiri-permissive-provenance
//! A kernel keeps a thread stack in an object. The thread switches away for
//! good, another thread frees the stack, the allocator hands the block out again
//! and a new thread runs on it: the old thread has ended.

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
    fn miri_switch_to_after_return(thread: usize);
}

use std::sync::atomic::{AtomicUsize, Ordering};

const STACK: usize = 0x10000;
static OLD: AtomicUsize = AtomicUsize::new(0);
static REAPER: AtomicUsize = AtomicUsize::new(0);
static STACK_PTR: AtomicUsize = AtomicUsize::new(0);
static DONE: AtomicUsize = AtomicUsize::new(0);

fn old_thread(_: usize) {
    let local = std::hint::black_box([1u64; 8]);
    assert_eq!(local[0], 1);
    // Switches away and is never resumed, as an exiting kernel thread does.
    unsafe { miri_switch_to(REAPER.load(Ordering::SeqCst)) };
    unreachable!();
}

fn new_thread(arg: usize) {
    let local = std::hint::black_box([arg as u64; 8]);
    DONE.store(local[7] as usize, Ordering::SeqCst);
}

fn reaper(_: usize) {
    unsafe {
        let stack = std::ptr::with_exposed_provenance_mut::<u8>(STACK_PTR.load(Ordering::SeqCst));
        kern_miri_object_died(stack);
        let stack = kern_miri_object_born(block(0), STACK, 16);
        let new = miri_create_new_thread(new_thread, 5, stack.addr() + STACK, STACK);
        // Hands over once this entry returns, so only the old thread is left
        // unfinished, and it has ended with its stack.
        miri_switch_to_after_return(new);
    }
}

fn main() {
    unsafe {
        let stack = kern_miri_object_born(block(0), STACK, 16);
        STACK_PTR.store(stack.expose_provenance(), Ordering::SeqCst);
        let old = miri_create_new_thread(old_thread, 0, stack.addr() + STACK, STACK);
        OLD.store(old, Ordering::SeqCst);
        REAPER.store(miri_create_new_thread(reaper, 0, 0x6000000, 0x100000), Ordering::SeqCst);
        miri_switch_to(old);
    }
    assert_eq!(DONE.load(Ordering::SeqCst), 5);
}
