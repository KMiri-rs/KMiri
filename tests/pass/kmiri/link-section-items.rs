//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance

//! KMiri lists the statics a kernel places in a link section, one at a time.
//! A linker script would lay them out as one array between two symbols; in
//! KMiri each static is its own allocation (see link-section-item-out-of-bounds).

#[used]
#[unsafe(link_section = ".kmiri_items")]
static FIRST: u32 = 1;
#[used]
#[unsafe(link_section = ".kmiri_items")]
static SECOND: u32 = 2;
#[used]
#[unsafe(link_section = ".kmiri_other")]
static OTHER: u32 = 3;

unsafe extern "Rust" {
    fn kern_miri_link_section_item(name: *const u8, name_len: usize, index: usize) -> *const ();
}

fn items(section: &str) -> Vec<*const u32> {
    let mut items = Vec::new();
    loop {
        let item =
            unsafe { kern_miri_link_section_item(section.as_ptr(), section.len(), items.len()) };
        if item.is_null() {
            return items;
        }
        items.push(item.cast::<u32>());
    }
}

fn main() {
    let found = items(".kmiri_items");
    // The items are the statics themselves, not copies.
    assert_eq!(found.len(), 2);
    assert!(found.contains(&(&raw const FIRST)));
    assert!(found.contains(&(&raw const SECOND)));
    let mut values: Vec<u32> = found.iter().map(|&item| unsafe { item.read() }).collect();
    values.sort();
    assert_eq!(values, [1, 2]);

    assert_eq!(items(".kmiri_other"), [&raw const OTHER]);
    assert!(items(".kmiri_empty").is_empty());
}
