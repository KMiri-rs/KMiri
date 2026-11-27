//@compile-flags: -Zkmiri-toml=dev/test_utils/pool.toml -Zmiri-permissive-provenance
//! An object covers only its own bytes: the allocator still reaches the pool
//! right before and after it through addresses (e.g. its block headers), and an
//! address inside the object resolves to the object.

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
        block(0xf8).cast::<u64>().write(1);
        block(0x110).cast::<u64>().write(2);
        assert_eq!(block(0xf8).cast::<u64>().read() + block(0x110).cast::<u64>().read(), 3);
        // Exposed by the announcement, so the address alone reaches the object.
        assert_eq!(block(0x100).cast::<u64>().read(), 7);
    }
}
