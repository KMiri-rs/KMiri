//@compile-flags: -Zkmiri-toml=dev/test_utils/pool.toml -Zmiri-permissive-provenance
//! Freeing an object twice is reported at the second free.

unsafe extern "Rust" {
    fn kern_miri_object_born(ptr: *mut u8, size: usize, align: usize) -> *mut u8;
    fn kern_miri_object_died(ptr: *mut u8) -> *mut u8;
}

const POOL: usize = 0x2000000;

/// The block at `POOL + offset`, reached through its address as an allocator does.
fn block(offset: usize) -> *mut u8 {
    std::ptr::with_exposed_provenance_mut(POOL + offset)
}

fn main() {
    unsafe {
        let object = kern_miri_object_born(block(0x100), 16, 8);
        kern_miri_object_died(object);
        kern_miri_object_died(object); //~ ERROR: not a live object
    }
}
