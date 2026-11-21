//! Checks the ktest runner under KMiri (`tool/ktest-sweep.py`).
//!
//! The four ktests below, run with `cargo osdk miri` through the sweep script,
//! must be recorded as: `a_passes` ok, `b_fails` FAILED, `c_use_after_free`
//! ABORT with Undefined Behavior, `d_passes_after_ub` ok. The last one shows
//! that the sweep restarts the kernel after the test that hit UB and still runs
//! the tests that come after it.
#![no_std]

extern crate alloc;

#[cfg(ktest)]
mod test {
    use alloc::boxed::Box;

    use ostd::prelude::*;

    #[ktest]
    fn a_passes() {
        assert_eq!(1 + 1, 2);
    }

    #[ktest]
    fn b_fails() {
        assert_eq!(1 + 1, 3, "this test is meant to fail");
    }

    #[ktest]
    fn c_use_after_free() {
        let boxed = Box::new(42u64);
        let ptr = &raw const *boxed;
        drop(boxed);
        // SAFETY: Not safe: `ptr` dangles. KMiri must report this read.
        let _ = unsafe { ptr.read() };
    }

    #[ktest]
    fn d_passes_after_ub() {
        assert!(Box::new(7u8).as_ref() == &7);
    }
}
