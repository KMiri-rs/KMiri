// The OSTD allocator statics are defined in a dependency crate, as in the full
// Asterinas kernel. Miri used to ICE at startup here: looking the statics up
// loaded their `mir_for_ctfe`, which only the defining crate has.
#[allow(unused_extern_crates)] // linked only for its exported statics
extern crate kmiri_alloc_static_dep;

unsafe extern "Rust" {
    static __GLOBAL_HEAP_ALLOCATOR_REF: i32;
    static __GLOBAL_FRAME_ALLOCATOR_REF: i32;
}

fn main() {
    unsafe {
        assert_eq!(__GLOBAL_HEAP_ALLOCATOR_REF, 7);
        assert_eq!(__GLOBAL_FRAME_ALLOCATOR_REF, 8);
    }
}
