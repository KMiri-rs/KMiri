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
    // Initialize the old extent so only a failure to leave the extension
    // uninitialized could make the read below pass.
    unsafe {
        let ptr = krealloc_node_align(std::ptr::null(), 8, 8, 0, -1);
        ptr.write_bytes(42, 8);
        let ptr = krealloc_node_align(ptr.cast(), 16, 8, 0, -1);
        assert_eq!(ptr.read(), 42);
        std::hint::black_box(ptr.add(8).read()); //~ ERROR: uninitialized
    }
}
