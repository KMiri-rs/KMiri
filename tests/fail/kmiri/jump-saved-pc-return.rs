//@compile-flags: -Zkmiri-toml=dev/test_utils/no-paging.toml -Zmiri-permissive-provenance
//! A CPU jump sets no return address: returning from the function reached by
//! `kern_miri_jump` is undefined behavior.

unsafe extern "Rust" {
    fn kern_miri_jump(pc: usize, regs: *const usize, nregs: usize) -> !;
}

extern "C" fn entry() {}

fn main() {
    unsafe { kern_miri_jump(entry as *const () as usize, std::ptr::null(), 0) } //~ ERROR: entering unreachable code
}
