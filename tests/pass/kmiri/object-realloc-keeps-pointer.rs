//@compile-flags: -Zkmiri-toml=dev/test_utils/pool.toml -Zmiri-permissive-provenance
//! A pointer stored in an object still points to its target after the object
//! moves, although the allocator copied it through the pool (where it carries
//! no provenance).

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

static TARGET: u64 = 42;

fn main() {
    unsafe {
        let old = kern_miri_object_born(block(0x100), 8, 8).cast::<&u64>();
        old.write(&TARGET);
        let raw = kern_miri_object_realloc_begin(old.cast());
        std::ptr::copy_nonoverlapping(raw, block(0x400), 8);
        let new = kern_miri_object_realloc_end(old.cast(), block(0x400), 16, 8).cast::<&u64>();
        assert_eq!(*new.read(), 42);
    }
}
