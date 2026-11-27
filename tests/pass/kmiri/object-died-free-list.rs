//@compile-flags: -Zkmiri-toml=dev/test_utils/pool.toml -Zmiri-permissive-provenance
//! The pointer returned when an object dies only carries its address; the
//! allocator links the free block through it into its free list.

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
        object.cast::<u64>().write(7);
        let free = kern_miri_object_died(object).cast::<usize>();
        free.write(POOL + 0x200);
        assert_eq!(free.read(), POOL + 0x200);
    }
}
