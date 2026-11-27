//@compile-flags: -Zkmiri-toml=dev/test_utils/pool.toml -Zmiri-permissive-provenance
//! A reallocation that moves the block: the allocator copies the bytes
//! through the pool (between `begin` and `end` the old block is pool memory to
//! it), and the new object keeps what the old one held.

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
        let old = kern_miri_object_born(block(0x100), 16, 8);
        old.cast::<u64>().write(7);
        old.cast::<u64>().add(1).write(8);
        let raw = kern_miri_object_realloc_begin(old);
        std::ptr::copy_nonoverlapping(raw, block(0x400), 16);
        let new = kern_miri_object_realloc_end(old, block(0x400), 64, 8).cast::<u64>();
        new.add(7).write(9);
        assert_eq!(new.read() + new.add(1).read() + new.add(7).read(), 24);
    }
}
