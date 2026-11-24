//@compile-flags: -Zkmiri-toml=dev/test_utils/virtio-blk.toml -Zmiri-permissive-provenance

//! A device writes by DMA wherever the driver tells it to. Writing an immutable
//! static is undefined behavior whoever writes, so a read request whose data
//! buffer is an immutable static is reported where the driver notifies the
//! device (Asterinas issue #3905).

#[path = "../../../dev/test_utils/virtio_blk.rs"]
mod utils;

use utils::*;

static VICTIM: [u8; 512] = [0; 512];

fn main() {
    unsafe {
        init_device();
        // The static's address is not exposed: the device needs no pointer.
        let victim = (&raw const VICTIM).addr() - KERNEL;
        submit(T_IN, 1, victim);
        kern_miri_mmio_write(at(LINEAR + DEVICE + 0x050), 4, 0); //~ ERROR: read-only
    }
}
