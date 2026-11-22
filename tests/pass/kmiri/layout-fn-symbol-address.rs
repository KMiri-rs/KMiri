//@compile-flags: -Zkmiri-toml=tests/pass/init_array_symbols.toml

//! A `[layout]` symbol declared as an `extern` function is a function at the
//! symbol address. rustc checks that the input of a function-pointer cast is a
//! real function, so the symbol keeps its function allocation, placed at the
//! address from `kmiri.toml`. Two declarations of one symbol agree on it.

mod a {
    unsafe extern "C" {
        pub fn __sinit_array();
    }
}

mod b {
    unsafe extern "C" {
        pub fn __sinit_array();
    }
}

fn main() {
    let fa: unsafe extern "C" fn() = a::__sinit_array;
    let fb: unsafe extern "C" fn() = b::__sinit_array;
    assert_eq!(fa as *const () as usize, 0x80014000);
    assert_eq!(fb as usize, 0x80014000);
    assert_eq!((fa as *const ()).addr(), (fb as *const ()).addr());
}
