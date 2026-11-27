//@compile-flags: -Zkmiri-toml=dev/test_utils/pool.toml -Zmiri-permissive-provenance
//! A new object is uninitialized, even if its block held data before.

unsafe extern "Rust" {
    fn kern_miri_object_born(ptr: *mut u8, size: usize, align: usize) -> *mut u8;
}

const POOL: usize = 0x2000000;

/// The block at `POOL + offset`, reached through its address as an allocator does.
fn block(offset: usize) -> *mut u8 {
    std::ptr::with_exposed_provenance_mut(POOL + offset)
}

fn main() {
    unsafe {
        block(0x100).cast::<u64>().write(7);
        let object = kern_miri_object_born(block(0x100), 8, 8).cast::<u64>();
        let _ = object.read(); //~ ERROR: uninitialized
    }
}
