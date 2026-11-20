//@compile-flags: -Zkmiri-toml=dev/test_utils/no-paging.toml

use std::sync::atomic::{AtomicUsize, Ordering};

static SECOND: AtomicUsize = AtomicUsize::new(0);
static PHASE: AtomicUsize = AtomicUsize::new(0);

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

struct Finish;
struct FinishTls;

thread_local! {
    static TLS: FinishTls = const { FinishTls };
}

impl Drop for Finish {
    fn drop(&mut self) {
        assert_eq!(PHASE.swap(2, Ordering::SeqCst), 1);
    }
}

impl Drop for FinishTls {
    fn drop(&mut self) {
        assert_eq!(PHASE.swap(3, Ordering::SeqCst), 2);
    }
}

fn first(_: usize) {
    let _finish = Finish;
    TLS.with(|_| {});
    // The successor must wait for both the entry's Drop and its TLS destructors.
    unsafe { miri_switch_to_after_return(SECOND.load(Ordering::SeqCst)) };
    assert_eq!(PHASE.swap(1, Ordering::SeqCst), 0);
}

fn second(_: usize) {
    assert_eq!(PHASE.swap(4, Ordering::SeqCst), 3);
}

fn main() {
    // Non-overlapping stacks model two kernel tasks; each is switched to by the
    // thread id the shim returned, as OSTD keeps it in the task context.
    // Both entries must finish; suppressing thread/leak checks would hide the bug.
    unsafe {
        let first_id = miri_create_new_thread(first, 0, 0x7000000, 0x100000);
        SECOND.store(miri_create_new_thread(second, 0, 0x7200000, 0x100000), Ordering::SeqCst);
        miri_switch_to(first_id);
    }
    assert_eq!(PHASE.load(Ordering::SeqCst), 4);
}
