//! Control for KMiri's `_Unwind_RaiseException` model: with no `catch_unwind`
//! on the stack, `ostd::panic::begin_panic` does not unwind at all.
//!
//! When the unwinder's first phase walks the whole stack without finding a
//! handler, it returns `END_OF_STACK` (5) and runs no cleanup. The KMiri entry
//! path (`miri_main`) has no `catch_unwind` frame, so that is what must happen
//! here. (A native ktest kernel is different: its test runner wraps every test
//! in `catch_unwind`.) The test holds a drop guard across the call and checks
//! that the call returns 5 and the guard is still alive afterwards.
//!
//! Expected: no Undefined Behavior, and the kernel exits normally.
#![no_std]
#![deny(unsafe_code)]
#![cfg_attr(miri, feature(format_args_nl))]

extern crate alloc;

use alloc::boxed::Box;
use core::sync::atomic::{AtomicBool, Ordering};

// Diagnostics only; compiled out outside Miri.
macro_rules! note {
    ($($t:tt)*) => { #[cfg(miri)] ostd::miri_println!($($t)*); };
}

static GUARD_DROPPED: AtomicBool = AtomicBool::new(false);

struct Guard;

impl Drop for Guard {
    fn drop(&mut self) {
        GUARD_DROPPED.store(true, Ordering::Relaxed);
    }
}

#[cfg_attr(miri, ostd::ktest::miri_main)]
#[cfg_attr(not(miri), ostd::main)]
fn kernel_main() {
    let guard = Guard;
    let code = ostd::panic::begin_panic(Box::new(42u32));
    assert_eq!(code.0, 5, "expected END_OF_STACK");
    assert!(!GUARD_DROPPED.load(Ordering::Relaxed));
    drop(guard);
    note!("unwind-no-handler: begin_panic returned END_OF_STACK");
}
