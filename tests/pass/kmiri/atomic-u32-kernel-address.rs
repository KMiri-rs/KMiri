//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance

//! `kern_miri_atomic_load_u32` and `kern_miri_atomic_cmpxchg_u32` are one atomic
//! step at a kernel address, as a kernel's fallible atomics are (OSTD's
//! `__atomic_load_fallible` / `__atomic_cmpxchg_fallible`).
//!
//!   SRC+8   untyped page: load, matching and mismatching compare-exchange
//!   a Rust `u32`, through an address cast back from its exposed pointer
//!
//! Expected: a matching compare-exchange stores the new value, a mismatching one
//! leaves the word alone, both return the old value, and the neighbouring bytes
//! are untouched.

#[path = "../../../dev/test_utils/physical_copy.rs"]
mod utils;

use utils::*;

extern "Rust" {
    fn kern_miri_atomic_load_u32(ptr: *const u32) -> u32;
    fn kern_miri_atomic_cmpxchg_u32(ptr: *mut u32, old_val: u32, new_val: u32) -> u32;
}

fn main() {
    unsafe {
        prepare_paging();
        kern_miri_alloc_pages(SRC, 1);
        kern_miri_zero(SRC, 1);

        let p = at(LINEAR + SRC + 8).cast::<u32>();
        p.write_volatile(5);
        assert_eq!(kern_miri_atomic_load_u32(p), 5);
        assert_eq!(kern_miri_atomic_cmpxchg_u32(p, 5, 6), 5);
        assert_eq!(kern_miri_atomic_cmpxchg_u32(p, 5, 7), 6);
        assert_eq!(p.read_volatile(), 6);
        assert_eq!(at(LINEAR + SRC + 4).cast::<u32>().read_volatile(), 0);
        assert_eq!(at(LINEAR + SRC + 12).cast::<u32>().read_volatile(), 0);

        let mut word = 5u32;
        let w = core::ptr::with_exposed_provenance_mut::<u32>((&raw mut word).expose_provenance());
        assert_eq!(kern_miri_atomic_cmpxchg_u32(w, 5, 6), 5);
        assert_eq!(kern_miri_atomic_load_u32(w), 6);
        assert_eq!(word, 6);
    }
}
