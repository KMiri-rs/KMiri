//@only-target: linux

use std::ffi::{c_int, c_uint, c_ulong, c_void};

unsafe extern "C" {
    fn krealloc_node_align(
        ptr: *const c_void,
        size: usize,
        align: c_ulong,
        flags: c_uint,
        nid: c_int,
    ) -> *mut u8;
}

fn main() {
    // Miri models realloc by moving the allocation. Retaining the original
    // pointer must not bypass liveness checking after a successful resize.
    unsafe {
        let old = krealloc_node_align(std::ptr::null(), 8, 8, 0, -1);
        old.write_bytes(42, 8);
        let new = krealloc_node_align(old.cast(), 32, 8, 0, -1);
        assert_eq!(new.read(), 42);
        std::hint::black_box(old.read()); //~ ERROR: has been freed
    }
}
