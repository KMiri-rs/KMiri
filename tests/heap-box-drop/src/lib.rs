//! Dropping a heap `Box` must be fine under KMiri, whether the `Box` goes out
//! of scope or is moved into a function such as `core::mem::drop`.
//!
//! A `Box` passed by value is weakly protected for the duration of the call;
//! Miri allows deallocating it through its own pointer during that time.
#![no_std]
#![deny(unsafe_code)]
#![feature(format_args_nl)]

extern crate alloc;

use alloc::boxed::Box;

struct Payload {
    value: u64,
}

#[ostd::ktest::miri_main]
fn kernel_main() {
    {
        let scoped = Box::new(Payload { value: 0xAA });
        assert_eq!(scoped.value, 0xAA);
    }
    ostd::miri_println!("heap-box-drop: scope drop ok");

    let moved = Box::new(Payload { value: 0xBB });
    assert_eq!(moved.value, 0xBB);
    core::mem::drop(moved);
    ostd::miri_println!("heap-box-drop: mem::drop ok");
}
