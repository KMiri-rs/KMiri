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
    // __GFP_ZERO is bit 8 in Linux include/linux/gfp_types.h. A fresh zeroed
    // allocation must be readable without initializing it on the Rust side.
    unsafe {
        // Other GFP bits may accompany __GFP_ZERO.
        let ptr =
            rust_helper_krealloc_node_align(std::ptr::null(), 16, 8, (1 << 8) | (1 << 10), -1);
        assert!(!ptr.is_null());
        for i in 0..16 {
            assert_eq!(ptr.add(i).read(), 0);
        }
        rust_helper_krealloc_node_align(ptr.cast(), 0, 1, 0, -1);
    }
}
