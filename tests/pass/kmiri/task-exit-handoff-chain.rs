//@compile-flags: -Zkmiri-toml=dev/test_utils/no-paging.toml

use std::sync::atomic::{AtomicUsize, Ordering};

const TASKS: usize = 3;
static THREADS: [AtomicUsize; TASKS] = [const { AtomicUsize::new(0) }; TASKS];
static FINISHED: AtomicUsize = AtomicUsize::new(0);

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

fn entry(index: usize) {
    assert_eq!(FINISHED.load(Ordering::SeqCst), index);
    if index + 1 < TASKS {
        // Each exiting task hands off only after its own entry returns.
        unsafe { miri_switch_to_after_return(THREADS[index + 1].load(Ordering::SeqCst)) };
    }
    assert_eq!(FINISHED.fetch_add(1, Ordering::SeqCst), index);
}

fn main() {
    // A chain catches stale handoff state; the final task has no successor.
    unsafe {
        for index in 0..TASKS {
            let thread =
                miri_create_new_thread(entry, index, 0x7000000 + index * 0x200000, 0x100000);
            THREADS[index].store(thread, Ordering::SeqCst);
        }
        miri_switch_to(THREADS[0].load(Ordering::SeqCst));
    }
    assert_eq!(FINISHED.load(Ordering::SeqCst), TASKS);
}
