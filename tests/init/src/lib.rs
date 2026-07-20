#![no_std]
#![deny(unsafe_code)]
#![feature(format_args_nl)]
#![feature(stmt_expr_attributes)]

extern crate alloc;
use ostd::prelude::*;

#[ostd::ktest::miri_main]
fn kernel_main() {
    println!("Hello world from guest kernel!");

    let task =
        ostd::task::TaskOptions::new(|| ostd::miri_println!("A custom task from miri_main!"))
            .spawn()
            .unwrap();
    ostd::miri_println!("miri_main task schedule_info={:?}", task.schedule_info());
}
