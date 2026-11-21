//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance

//! Each item of a link section is its own allocation in KMiri, so stepping
//! from one item to the next, as code walking a linker-built array would,
//! is reported.

#[used]
#[unsafe(link_section = ".kmiri_items")]
static FIRST: u32 = 1;
#[used]
#[unsafe(link_section = ".kmiri_items")]
static SECOND: u32 = 2;

unsafe extern "Rust" {
    fn kern_miri_link_section_item(name: *const u8, name_len: usize, index: usize) -> *const ();
}

fn main() {
    let section = ".kmiri_items";
    let first = unsafe { kern_miri_link_section_item(section.as_ptr(), section.len(), 0) };
    let _ = unsafe { first.cast::<u32>().add(1).read() }; //~ ERROR: beyond the end of the allocation of size 4 bytes
}
