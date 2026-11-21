//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2.toml
//! Only CPU 0 runs at reset. A started CPU runs its entry on its own, and sees
//! everything CPU 0 wrote before starting it.

#[path = "../../../dev/test_utils/smp.rs"]
mod smp;

use std::cell::UnsafeCell;
use std::hint::spin_loop;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Plain(UnsafeCell<usize>);
unsafe impl Sync for Plain {}

static DATA: Plain = Plain(UnsafeCell::new(0));
static SEEN: AtomicUsize = AtomicUsize::new(0);

fn entry(arg: usize) {
    assert_eq!(smp::current(), 1);
    // Written by CPU 0 before it started this CPU: no race.
    let data = unsafe { *DATA.0.get() };
    SEEN.store(data + arg, Ordering::Release);
}

fn main() {
    assert_eq!(smp::count(), 2);
    assert_eq!(smp::current(), 0);
    unsafe { *DATA.0.get() = 40 };
    smp::start(1, entry, 2);
    while SEEN.load(Ordering::Acquire) == 0 {
        spin_loop();
    }
    assert_eq!(SEEN.load(Ordering::Acquire), 42);
    assert_eq!(smp::current(), 0);
}
