//! OSTD's time-stamp counter must be readable under KMiri.
//!
//! The full Asterinas kernel reads it while the `aster-time` component
//! calibrates its clock source; `read_tsc` used to be `unimplemented!()` in the
//! KMiri arch. It now reads KMiri's virtual clock, which counts nanoseconds,
//! so the reported frequency is 1 GHz.
//!
//! Expected: the counter does not go backwards and the kernel exits normally.
#![no_std]
#![deny(unsafe_code)]
#![feature(format_args_nl)]

extern crate alloc;

use ostd::arch::{read_tsc, tsc_freq};

#[ostd::ktest::miri_main]
fn kernel_main() {
    assert_eq!(tsc_freq(), 1_000_000_000);
    let first = read_tsc();
    let second = read_tsc();
    assert!(second >= first);
    ostd::miri_println!("tsc-read: ok");
}
