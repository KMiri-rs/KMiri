//@compile-flags: -Zkmiri-toml=tests/fail/kmiri/cpu-local-section-full.toml -Zmiri-permissive-provenance
//! A CPU-local variable that does not fit in the section is reported as such, with
//! the size to configure. Its alignment needs more than the default 64 KiB.

#[repr(align(0x10000))]
struct Big(#[allow(dead_code)] u64);

#[unsafe(link_section = ".cpu_local")]
static BIG: Big = Big(7);

fn main() {
    let _ = &BIG as *const Big as usize; //~ ERROR: the CPU-local section is full
}
