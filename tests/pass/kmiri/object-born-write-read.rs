//@compile-flags: -Zkmiri-toml=dev/test_utils/pool.toml -Zmiri-permissive-provenance
//! After the allocator hands out a block, the object starts uninitialized and
//! the returned pointer reads back what was written through it.

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
        let object = kern_miri_object_born(block(0x100), 16, 8).cast::<u64>();
        object.write(7);
        object.add(1).write(8);
        assert_eq!(object.read() + object.add(1).read(), 15);
    }
}
