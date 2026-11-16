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
    // __GFP_DIRECT_RECLAIM (bit 10) is not __GFP_ZERO: a nonzero flags word
    // alone must not initialize a fresh allocation.
    unsafe {
        let ptr = krealloc_node_align(std::ptr::null(), 8, 8, 1 << 10, -1);
        std::hint::black_box(ptr.read()); //~ ERROR: uninitialized
    }
}
