//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance
#![feature(linkage)]

//! As with the linker, a strong definition of a symbol overrides a weak one of
//! the same name. Kernels use this to give a default (weak) entry point that a
//! kernel's own entry point replaces.

mod default_entry {
    #[linkage = "weak"]
    #[unsafe(no_mangle)]
    pub static KMIRI_ENTRY_NAME: &str = "default";
}

mod kernel_entry {
    #[unsafe(no_mangle)]
    pub static KMIRI_ENTRY_NAME: &str = "kernel";
}

unsafe extern "Rust" {
    static KMIRI_ENTRY_NAME: &'static str;
}

fn main() {
    assert_eq!(unsafe { KMIRI_ENTRY_NAME }, "kernel");
}
