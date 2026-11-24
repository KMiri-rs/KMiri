//@compile-flags: -Zkmiri-toml=dev/test_utils/virtio-blk.toml -Zmiri-permissive-provenance

//! KMiri models no memory a device could reach outside RAM, so a DMA access
//! there is not supported.

#[path = "../../../dev/test_utils/virtio_blk.rs"]
mod utils;

use utils::*;

fn main() {
    unsafe {
        init_device();
        submit(T_IN, 1, 0x0800_0000);
        kern_miri_mmio_write(at(LINEAR + DEVICE + 0x050), 4, 0); //~ ERROR: outside RAM
    }
}
