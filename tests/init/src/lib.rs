#![no_std]
#![deny(unsafe_code)]
#![feature(format_args_nl)]

extern crate alloc;
use ostd::prelude::*;

#[ostd::ktest::miri_main]
fn kernel_main() {
    println!("Hello world from guest kernel!");
}
