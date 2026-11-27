//@compile-flags: -Zkmiri-toml=dev/test_utils/pool.toml -Zmiri-permissive-provenance
//! Three layers at one place: a thread local inside the thread stack inside an
//! object inside the pool. An address resolves to the innermost live one: the
//! local, then the object (a word the kernel keeps at the top of the stack),
//! then the pool.

unsafe extern "Rust" {
    fn kern_miri_object_born(ptr: *mut u8, size: usize, align: usize) -> *mut u8;
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

static SEEN: AtomicUsize = AtomicUsize::new(0);

fn entry(saved: usize) {
    let local = std::hint::black_box([1u64; 8]);
    let addr = (&raw const local).expose_provenance();
    unsafe {
        let via_local = std::ptr::with_exposed_provenance::<u64>(addr).read();
        let via_object = std::ptr::with_exposed_provenance::<u64>(saved).read();
        let via_pool = block(0x30000).cast::<u64>().read();
        SEEN.store((via_local + via_object + via_pool) as usize, Ordering::SeqCst);
    }
}

fn main() {
    unsafe {
        let object = kern_miri_object_born(block(0), 0x20000, 16);
        // The stack range ends below the word the kernel keeps at its top.
        let saved = object.add(0x18000 - 16).cast::<u64>();
        saved.write(10);
        block(0x30000).cast::<u64>().write(100);
        let saved = saved.expose_provenance();
        let thread = miri_create_new_thread(entry, saved, saved, 0x10000 - 16);
        miri_switch_to(thread);
    }
    assert_eq!(SEEN.load(Ordering::SeqCst), 111);
}
