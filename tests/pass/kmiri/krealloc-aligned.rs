//@compile-flags: -Zmiri-symbolic-alignment-check
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

#[repr(align(8192))]
struct Aligned(u8);

fn main() {
    // Like RFL's Kmalloc::aligned_layout, request a size padded to alignment.
    // A typed access checks allocation alignment, not accidental address bits.
    unsafe {
        let ptr = krealloc_node_align(std::ptr::null(), size_of::<Aligned>(), 8192, 0, -1);
        let aligned = ptr.cast::<Aligned>();
        aligned.write(Aligned(42));
        assert_eq!((*aligned).0, 42);
        krealloc_node_align(ptr.cast(), 0, 1, 0, -1);
    }
}
