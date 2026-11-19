//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance

//! Writing through an integer address that no page table maps must be reported
//! as UB, not crash the interpreter.
//!
//! This is what Asterinas #1069 does: a CPU-local variable is accessed before
//! the CPU-local base register is set, so its address is `0 + offset`, a small
//! integer that belongs to no allocation. The pointer comes from an
//! integer-to-pointer cast, so it carries wildcard provenance and KMiri must
//! translate the address before it can look up the allocation.

fn main() {
    let addr = 0x8usize as *mut u64;
    unsafe { addr.write(1) }; //~ ERROR: dangling pointer
}
