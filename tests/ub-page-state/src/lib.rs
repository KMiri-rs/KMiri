#![no_std]
#![feature(format_args_nl, stmt_expr_attributes)]

extern crate alloc;

#[ostd::ktest::miri_main]
fn kernel_main() {
    use ostd::arch::PageType;
    use ostd::arch::kern_miri_alloc_pages;
    use ostd::task::TaskOptions;

    const PAGE: usize = 0x7000000;
    let task = TaskOptions::new(|| unsafe {
        kern_miri_alloc_pages(PAGE, 1);
        kern_miri_alloc_pages(PAGE, 1);
    })
    .spawn()
    .unwrap();
    ostd::task::Task::yield_now();
}
