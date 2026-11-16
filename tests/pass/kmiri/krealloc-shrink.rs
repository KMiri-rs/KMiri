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
    // Even with __GFP_ZERO, shrinking preserves every byte in the new extent.
    unsafe {
        let ptr = krealloc_node_align(std::ptr::null(), 32, 8, 1 << 8, -1);
        for i in 0..32 {
            ptr.add(i).write(i as u8 + 1);
        }
        let ptr = krealloc_node_align(ptr.cast(), 16, 8, 1 << 8, -1);
        for i in 0..16 {
            assert_eq!(ptr.add(i).read(), i as u8 + 1);
        }
        krealloc_node_align(ptr.cast(), 0, 1, 0, -1);
    }
}
