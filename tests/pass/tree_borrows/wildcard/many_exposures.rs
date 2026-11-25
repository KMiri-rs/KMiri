//@compile-flags: -Zmiri-tree-borrows -Zmiri-permissive-provenance

// Regression test for rust-lang/miri#5673: exposing 2^16+ tags on the same
// allocation used to overflow u16 local_writes/local_reads counters.
fn main() {
    let mut x = 0u8;
    let mut addr = 0usize;
    for _ in 0..65_536u32 {
        addr = (&mut x as *mut u8).expose_provenance();
    }
    let p = std::ptr::with_exposed_provenance_mut::<u8>(addr);
    unsafe { *p = 1 };
    assert_eq!(x, 1);
}
