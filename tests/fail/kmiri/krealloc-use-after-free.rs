//@only-target: linux

use std::ffi::{c_int, c_uint, c_ulong, c_void};

unsafe extern "C" {
    fn rust_helper_krealloc_node_align(
        ptr: *const c_void,
        size: usize,
        align: c_ulong,
        flags: c_uint,
        nid: c_int,
    ) -> *mut u8;
}

fn main() {
    // RFL frees via a zero-sized realloc. The old pointer must be dead even
    // when the allocation was initialized, so this specifically checks liveness.
    unsafe {
        let ptr = rust_helper_krealloc_node_align(std::ptr::null(), 8, 8, 0, -1);
        ptr.write(42);
        rust_helper_krealloc_node_align(ptr.cast(), 0, 1, 0, -1);
        std::hint::black_box(ptr.read()); //~ ERROR: has been freed
    }
}
