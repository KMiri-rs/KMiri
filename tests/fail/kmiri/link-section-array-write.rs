//@compile-flags: -Zkmiri-toml=dev/test_utils/link-section-array.toml

//! The array of a configured link section is read-only.

#[used]
#[unsafe(link_section = ".kmiri_array")]
static FIRST: u32 = 1;

unsafe extern "C" {
    static __kmiri_array_start: u32;
}

fn main() {
    let first = (&raw const __kmiri_array_start).cast_mut();
    unsafe { first.write(5) }; //~ ERROR: read-only
}
