//@compile-flags: -Zkmiri-toml=dev/test_utils/no-paging.toml -Zmiri-permissive-provenance
//! A thread's stack range may lie in memory the program owns, as a kernel keeps
//! a thread's saved state at the top of its stack. Once the thread's locals sit
//! in that memory, an address no local covers still reaches the memory itself.

use std::sync::atomic::{AtomicUsize, Ordering};

unsafe extern "Rust" {
    fn miri_create_new_thread(
        func: fn(usize),
        arg: usize,
        stack_end: usize,
        stack_size: usize,
    ) -> usize;
    fn miri_switch_to(thread: usize);
}

const STACK_SIZE: usize = 0x10000;

#[repr(C, align(16))]
struct Stack([u8; STACK_SIZE]);

static mut STACK: Stack = Stack([0; STACK_SIZE]);
static SEEN: AtomicUsize = AtomicUsize::new(0);

fn entry(saved: usize) {
    let locals = std::hint::black_box([1u64; 8]);
    let value = unsafe { std::ptr::with_exposed_provenance::<u64>(saved).read() };
    SEEN.store(value as usize + locals[0] as usize, Ordering::SeqCst);
}

fn main() {
    let base = (&raw mut STACK).cast::<u8>();
    // The saved state occupies the top 16 bytes; the stack range ends below it.
    let saved = unsafe { base.add(STACK_SIZE - 16) }.cast::<u64>();
    unsafe { saved.write(41) };
    let saved = saved.expose_provenance();
    unsafe {
        let thread = miri_create_new_thread(entry, saved, saved, saved - base.addr());
        miri_switch_to(thread);
    }
    assert_eq!(SEEN.load(Ordering::SeqCst), 42);
}
