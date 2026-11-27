//@compile-flags: -Zkmiri-toml=dev/test_utils/link-section-array.toml

//! A link section configured in `[[link_section]]` is laid out as one array
//! between its start and end symbols, so a kernel can walk it element by
//! element as it does with a linker script.

use std::ptr::addr_of;
use std::sync::atomic::{AtomicUsize, Ordering};

#[used]
#[unsafe(link_section = ".kmiri_array")]
static FIRST: u32 = 1;
#[used]
#[unsafe(link_section = ".kmiri_array")]
static SECOND: u32 = 2;
#[used]
#[unsafe(link_section = ".kmiri_other")]
static OTHER: u32 = 3;

static CALLED: AtomicUsize = AtomicUsize::new(0);
extern "C" fn one() {
    CALLED.fetch_add(1, Ordering::Relaxed);
}
extern "C" fn ten() {
    CALLED.fetch_add(10, Ordering::Relaxed);
}
#[used]
#[unsafe(link_section = ".kmiri_calls")]
static CALL_ONE: extern "C" fn() = one;
#[used]
#[unsafe(link_section = ".kmiri_calls")]
static CALL_TEN: extern "C" fn() = ten;

unsafe extern "C" {
    static __kmiri_array_start: u32;
    static __kmiri_array_end: u32;
    static __kmiri_calls_start: extern "C" fn();
    static __kmiri_calls_end: extern "C" fn();
    static __kmiri_none_start: u32;
    static __kmiri_none_end: u32;
}

fn main() {
    let mut values = Vec::new();
    let mut item = addr_of!(__kmiri_array_start);
    while item < addr_of!(__kmiri_array_end) {
        values.push(unsafe { item.read() });
        item = unsafe { item.offset(1) };
    }
    values.sort();
    assert_eq!(values, [1, 2]);
    assert_eq!(OTHER, 3);

    // Function pointers keep their provenance in the copy.
    let mut call = addr_of!(__kmiri_calls_start);
    while call < addr_of!(__kmiri_calls_end) {
        unsafe { (*call)() };
        call = unsafe { call.offset(1) };
    }
    assert_eq!(CALLED.load(Ordering::Relaxed), 11);

    assert_eq!(addr_of!(__kmiri_none_start), addr_of!(__kmiri_none_end));
}
