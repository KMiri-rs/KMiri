//@compile-flags: -Zkmiri-toml=dev/test_utils/physical-copy.toml -Zmiri-permissive-provenance

//! `kern_miri_copy` between two untyped ranges that both cross a page
//! boundary, at different points.
//!
//!   SRC+4000..SRC+4300 written (300 bytes)
//!   SRC+3990..SRC+4300 --copy--> DST+4000..DST+4310
//!   (the source crosses after 106 bytes, the destination after 96)
//!
//! Expected: DST+4010..DST+4310 holds the written bytes.

#[path = "../../../dev/test_utils/physical_copy.rs"]
mod utils;

use utils::*;

fn main() {
    unsafe {
        prepare_paging();
        kern_miri_alloc_pages(SRC, 2);
        kern_miri_alloc_pages(DST, 2);

        let data: [u8; 300] = core::array::from_fn(|i| (i % 251) as u8);
        kern_miri_write_bytes(SRC + 4000, data.as_ptr(), data.len());
        kern_miri_copy(at(LINEAR + DST + 4000), at(LINEAR + SRC + 3990), 310);

        let mut back = [0u8; 300];
        kern_miri_copy(back.as_mut_ptr().cast(), at(LINEAR + DST + 4010), back.len());
        assert_eq!(back, data);
    }
}
