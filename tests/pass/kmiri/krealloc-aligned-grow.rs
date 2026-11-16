//@compile-flags: -Zmiri-symbolic-alignment-check
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

#[repr(align(8192))]
struct Aligned(u8);

fn main() {
    // Changing alignment during reallocation must preserve the prefix and
    // support a typed access through the returned, more strongly aligned pointer.
    unsafe {
        let ptr = rust_helper_krealloc_node_align(std::ptr::null(), 16, 16, 0, -1);
        ptr.write(23);
        let ptr = rust_helper_krealloc_node_align(ptr.cast(), size_of::<Aligned>(), 8192, 0, -1);
        assert_eq!(ptr.read(), 23);
        let aligned = ptr.cast::<Aligned>();
        aligned.write(Aligned(42));
        assert_eq!((*aligned).0, 42);
        rust_helper_krealloc_node_align(ptr.cast(), 0, 1, 0, -1);
    }
}
