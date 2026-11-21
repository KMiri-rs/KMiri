//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2-paging.toml -Zmiri-permissive-provenance
//! A kernel gives each other CPU its own copy of the CPU-local section by copying
//! the section. `kern_miri_copy_cpu_local_section` does that copy with one
//! allocation per variable, so that each copy is memory Miri checks:
//!
//! - a variable used before the copy is copied with its value then;
//! - a variable first used after the copy still had its initial value then, and
//!   its copy holds that, whatever the original holds now;
//! - a pointer in a variable keeps its provenance in the copy;
//! - two CPUs that each write their own copy do not race.

#[path = "../../../dev/test_utils/smp.rs"]
mod smp;
#[path = "../../../dev/test_utils/physical_copy.rs"]
mod utils;

use std::cell::UnsafeCell;
use std::hint::spin_loop;
use std::sync::atomic::{AtomicBool, Ordering};

#[repr(transparent)]
struct Plain<T>(UnsafeCell<T>);
unsafe impl<T> Sync for Plain<T> {}

static TARGET: u32 = 42;

#[unsafe(link_section = ".cpu_local")]
static USED: Plain<usize> = Plain(UnsafeCell::new(1));
#[unsafe(link_section = ".cpu_local")]
static LATER: Plain<usize> = Plain(UnsafeCell::new(2));
#[unsafe(link_section = ".cpu_local")]
static COPY_ONLY: Plain<usize> = Plain(UnsafeCell::new(3));
#[unsafe(link_section = ".cpu_local")]
static PTR_USED: Plain<&u32> = Plain(UnsafeCell::new(&TARGET));
#[unsafe(link_section = ".cpu_local")]
static PTR_LATER: Plain<&u32> = Plain(UnsafeCell::new(&TARGET));

static DONE: AtomicBool = AtomicBool::new(false);

/// `var`'s value in the copy of the section at `base`.
fn at<T>(base: usize, var: &'static Plain<T>) -> *mut T {
    smp::local(base, var).cast()
}
/// Physical address of the copy, and its virtual address.
const COPY_PADDR: usize = 0x0200_0000;
const COPY: usize = utils::LINEAR + COPY_PADDR;
/// CPU 1 runs on a stack in the mapped window.
const CPU1_STACK_END: usize = utils::LINEAR + 0x0620_0000;

fn entry(base: usize) {
    unsafe {
        assert_eq!(*at(base, &USED), 5);
        assert_eq!(*at(base, &LATER), 2);
        assert_eq!(*at(base, &COPY_ONLY), 3);
        assert_eq!(**at(base, &PTR_USED), 42);
        assert_eq!(**at(base, &PTR_LATER), 42);
        *at(base, &USED) = 100;
    }
    DONE.store(true, Ordering::Release);
}

fn main() {
    unsafe {
        *USED.0.get() = 5;
        assert_eq!(**PTR_USED.0.get(), 42);
    }
    unsafe { utils::prepare_paging() };
    smp::copy_section(COPY_PADDR, COPY);
    unsafe { *LATER.0.get() = 9 };

    smp::start_with_stack(1, entry, COPY, CPU1_STACK_END);
    // CPU 1 writes its own copy of `USED` meanwhile.
    unsafe { *USED.0.get() = 6 };
    while !DONE.load(Ordering::Acquire) {
        spin_loop();
    }
    unsafe {
        assert_eq!(*USED.0.get(), 6);
        assert_eq!(*LATER.0.get(), 9);
        assert_eq!(*at(COPY, &USED), 100);
    }
}
