//@compile-flags: -Zkmiri-toml=dev/test_utils/clint.toml -Zmiri-permissive-provenance
//! An access that is not one whole CLINT register is reported.

fn main() {
    unsafe { std::ptr::read_volatile(0x2004002 as *const u16) }; //~ ERROR: is not one of its registers
}
