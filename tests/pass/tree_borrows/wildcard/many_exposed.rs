//@revisions: tree tree_implicit_writes
//@[tree_implicit_writes]compile-flags: -Zmiri-tree-borrows-implicit-writes
//@compile-flags: -Zmiri-tree-borrows -Zmiri-permissive-provenance

/// More than `u16::MAX` exposed references in one tree must not wrap the
/// per-node exposure counters. With 16-bit counters, 65536 exposures wrapped
/// the root's count to 0, and the wildcard write below was reported as UB
/// (or, with overflow checks, the interpreter panicked).
pub fn main() {
    let mut x = 0u8;
    let mut addr = 0;
    for _ in 0..65_536u32 {
        addr = (&mut x as *mut u8).expose_provenance();
    }
    let p = std::ptr::with_exposed_provenance_mut::<u8>(addr);
    unsafe { *p = 1 };
    assert_eq!(x, 1);
}
