//@compile-flags: -Zkmiri-toml=dev/test_utils/pool.toml -Zmiri-permissive-provenance
//! A reallocation that keeps the block: growing keeps the old bytes, then
//! shrinking keeps the bytes that still fit.

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
        let small = kern_miri_object_born(block(0x100), 16, 8);
        small.cast::<u64>().write(7);
        small.cast::<u64>().add(1).write(8);
        kern_miri_object_realloc_begin(small);
        let big = kern_miri_object_realloc_end(small, small, 32, 8).cast::<u64>();
        big.add(3).write(9);
        assert_eq!(big.read() + big.add(1).read() + big.add(3).read(), 24);
        kern_miri_object_realloc_begin(big.cast());
        let shrunk = kern_miri_object_realloc_end(big.cast(), big.cast(), 8, 8).cast::<u64>();
        assert_eq!(shrunk.read(), 7);
    }
}
