//! Control for KMiri's `_Unwind_RaiseException` model: an unwind started by
//! `ostd::panic::begin_panic` inside `ostd::panic::catch_unwind` is caught.
//!
//! The closure owns a drop guard and calls `begin_panic` with a boxed `42u32`.
//! The test checks that `catch_unwind` returns `Err` with that payload, that the
//! guard was dropped on the way out, and that the code after `begin_panic` did
//! not run.
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
static AFTER_BEGIN_PANIC: AtomicBool = AtomicBool::new(false);

struct Guard;

impl Drop for Guard {
    fn drop(&mut self) {
        GUARD_DROPPED.store(true, Ordering::Relaxed);
    }
}

#[cfg_attr(miri, ostd::ktest::miri_main)]
#[cfg_attr(not(miri), ostd::main)]
fn kernel_main() {
    let result = ostd::panic::catch_unwind(|| {
        let _guard = Guard;
        let _ = ostd::panic::begin_panic(Box::new(42u32));
        AFTER_BEGIN_PANIC.store(true, Ordering::Relaxed);
    });
    let payload = match result {
        Ok(()) => panic!("begin_panic returned instead of unwinding"),
        Err(payload) => payload,
    };
    assert_eq!(payload.downcast_ref::<u32>(), Some(&42));
    assert!(GUARD_DROPPED.load(Ordering::Relaxed));
    assert!(!AFTER_BEGIN_PANIC.load(Ordering::Relaxed));
    note!("unwind-catch: caught payload 42, guard dropped");
}
