//@compile-flags: -Zkmiri-toml=dev/test_utils/pool.toml -Zmiri-permissive-provenance
//! During a reallocation the allocator works on the block through the pointer
//! `begin` returns: it may touch bytes past the old object (growing in place
//! over a free neighbour) and, after moving, link the old block into a free
//! list. Those accesses reach the pool, not the old object.

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
        // Grow in place: the allocator clears the header of the free
        // neighbour right past the old object.
        let small = kern_miri_object_born(block(0x100), 16, 8);
        small.cast::<u64>().write(7);
        let raw = kern_miri_object_realloc_begin(small);
        raw.add(16).cast::<u64>().write(0);
        let big = kern_miri_object_realloc_end(small, raw, 32, 8).cast::<u64>();
        assert_eq!(big.read(), 7);

        // Move: after copying, the allocator links the old block into its
        // free list and later reads the link back through the pool.
        let raw = kern_miri_object_realloc_begin(big.cast());
        std::ptr::copy_nonoverlapping(raw, block(0x400), 32);
        raw.cast::<usize>().write(POOL + 0x800);
        let moved = kern_miri_object_realloc_end(big.cast(), block(0x400), 64, 8).cast::<u64>();
        assert_eq!(moved.read(), 7);
        assert_eq!(block(0x100).cast::<usize>().read(), POOL + 0x800);
    }
}
