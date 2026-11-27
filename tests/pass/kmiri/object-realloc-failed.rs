//@compile-flags: -Zkmiri-toml=dev/test_utils/pool.toml -Zmiri-permissive-provenance
//! A failed reallocation (`end` gets null) leaves the old object as it was.

unsafe extern "Rust" {
    fn kern_miri_object_born(ptr: *mut u8, size: usize, align: usize) -> *mut u8;
    fn kern_miri_object_died(ptr: *mut u8) -> *mut u8;
    fn kern_miri_object_realloc_begin(ptr: *mut u8) -> *mut u8;
    fn kern_miri_object_realloc_end(
        old: *mut u8,
        new: *mut u8,
        size: usize,
        align: usize,
    ) -> *mut u8;
}

const POOL: usize = 0x2000000;

/// The block at `POOL + offset`, reached through its address as an allocator does.
fn block(offset: usize) -> *mut u8 {
    std::ptr::with_exposed_provenance_mut(POOL + offset)
}

fn main() {
    unsafe {
        let old = kern_miri_object_born(block(0x100), 16, 8);
        old.cast::<u64>().write(7);
        kern_miri_object_realloc_begin(old);
        let new = kern_miri_object_realloc_end(old, std::ptr::null_mut(), 1 << 30, 8);
        assert!(new.is_null());
        assert_eq!(old.cast::<u64>().read(), 7);
        kern_miri_object_died(old);
    }
}
