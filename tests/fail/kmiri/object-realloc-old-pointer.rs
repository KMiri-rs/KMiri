//@compile-flags: -Zkmiri-toml=dev/test_utils/pool.toml -Zmiri-permissive-provenance
//! After a successful reallocation the old pointer is dangling, even when the
//! block stayed in place.

unsafe extern "Rust" {
    fn kern_miri_object_born(ptr: *mut u8, size: usize, align: usize) -> *mut u8;
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
        let old = kern_miri_object_born(block(0x100), 8, 8);
        kern_miri_object_realloc_begin(old);
        let _new = kern_miri_object_realloc_end(old, old, 16, 8);
        old.write(1); //~ ERROR: freed
    }
}
