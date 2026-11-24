//! `kern_miri_monotonic_nanos` is the clock that a kernel's arch layer reads
//! as its time-stamp counter (e.g. OSTD's `read_tsc`).
//!
//! It must never go backwards, and it must advance while the program runs.
//! With isolation (the default) it is Miri's virtual clock, which advances by
//! a fixed amount per basic block, so the result is the same on every run.
use std::hint::black_box;

extern "Rust" {
    fn kern_miri_monotonic_nanos() -> u64;
}

fn main() {
    let start = unsafe { kern_miri_monotonic_nanos() };
    let mut last = start;
    for i in 0..100u64 {
        black_box(i);
        let now = unsafe { kern_miri_monotonic_nanos() };
        assert!(now >= last);
        last = now;
    }
    assert!(last > start);
}
