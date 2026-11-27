//@compile-flags: -Zkmiri-toml=dev/test_utils/pool.toml -Zmiri-permissive-provenance
//! A pool is memory a kernel allocator manages. Before any object is
//! announced, the allocator reaches all of it through addresses, e.g. to keep
//! its free lists there.

const POOL: usize = 0x2000000;

/// The block at `POOL + offset`, reached through its address as an allocator does.
fn block(offset: usize) -> *mut u8 {
    std::ptr::with_exposed_provenance_mut(POOL + offset)
}

fn main() {
    unsafe {
        let first = block(0).cast::<usize>();
        let last = block(0x100000 - 8).cast::<usize>();
        first.write(1);
        last.write(2);
        assert_eq!(first.read() + last.read(), 3);
    }
}
