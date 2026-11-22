//@compile-flags: -Zkmiri-toml=tests/pass/init_array_symbols.toml

//! A `[layout]` symbol only gives an address; there is no code to run there.

unsafe extern "C" {
    fn __sinit_array();
}

fn main() {
    unsafe { __sinit_array() } //~ ERROR: __sinit_array
}
