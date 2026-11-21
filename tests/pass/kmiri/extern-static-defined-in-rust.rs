//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance

//! An `extern` static declared in Rust resolves to the exported static of the
//! same name defined elsewhere in the program, as the linker resolves it. OSDK
//! defines the ktest whitelists that OSTD declares this way.

mod definer {
    #[unsafe(no_mangle)]
    pub static KMIRI_LINKED_WHITELIST: Option<&[&str]> = Some(&["a::b", "c"]);
}

unsafe extern "Rust" {
    static KMIRI_LINKED_WHITELIST: Option<&'static [&'static str]>;
}

fn main() {
    let declared = &raw const KMIRI_LINKED_WHITELIST;
    // Both names refer to the same static, not to a copy.
    assert_eq!(declared, &raw const definer::KMIRI_LINKED_WHITELIST);
    assert_eq!(unsafe { KMIRI_LINKED_WHITELIST }, Some(&["a::b", "c"][..]));
}
