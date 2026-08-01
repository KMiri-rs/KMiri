#![no_std]
#![deny(unsafe_code)]
#![feature(format_args_nl, stmt_expr_attributes)]

extern crate alloc;

#[ostd::ktest::miri_main]
fn kernel_main() {
    ostd::miri_println!("Hello world from guest kernel!");

    let task =
        ostd::task::TaskOptions::new(|| ostd::miri_println!("A custom task from miri_main!"))
            .spawn()
            .unwrap();
    ostd::miri_println!("miri_main task schedule_info={:?}", task.schedule_info());
    ostd::miri_println!("main: before yield_now");
    ostd::task::Task::yield_now();
    ostd::miri_println!("main: after yield_now");
}
