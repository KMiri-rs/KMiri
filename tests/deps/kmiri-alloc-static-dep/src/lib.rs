//! Defines the OSTD allocator statics in a dependency crate, the way the full
//! Asterinas kernel does (`aster_core::vm`). Used by
//! `tests/pass-dep/kmiri/allocator-static-in-dep.rs`.

#[no_mangle]
pub static __GLOBAL_HEAP_ALLOCATOR_REF: i32 = 7;

#[no_mangle]
pub static __GLOBAL_FRAME_ALLOCATOR_REF: i32 = 8;
