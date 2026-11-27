//@compile-flags: -Zkmiri-toml=dev/test_utils/no-paging.toml -Zmiri-permissive-provenance
//! `kern_miri_jump` continues at a saved program counter: the function takes as
//! many argument registers as it declares, and a register holding an address
//! becomes a pointer.

unsafe extern "Rust" {
    fn kern_miri_jump(pc: usize, regs: *const usize, nregs: usize) -> !;
}

extern "C" fn add_one(value: *mut u32) {
    unsafe { *value += 1 };
}

extern "C" fn entry(f: extern "C" fn(*mut u32), value: *mut u32, small: u8) -> ! {
    f(value);
    println!("value {} small {}", unsafe { *value }, small);
    std::process::exit(0)
}

fn main() {
    let value = Box::into_raw(Box::new(41u32));
    let regs: [usize; 8] = [
        add_one as *const () as usize,
        value.expose_provenance(),
        0x1_07, // only the low byte reaches a `u8` parameter
        0xdead,
        0,
        0,
        0,
        0,
    ];
    unsafe { kern_miri_jump(entry as *const () as usize, regs.as_ptr(), regs.len()) }
}
