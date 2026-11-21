//@error-in-other-file: the program tried to start CPU 1, which is already running
//@compile-flags: -Zkmiri-toml=dev/test_utils/smp2.toml
//! Starting a CPU that is already running stops the program.

#[path = "../../../dev/test_utils/smp.rs"]
mod smp;

fn entry(_: usize) {
    loop {
        std::hint::spin_loop();
    }
}

fn main() {
    smp::start(1, entry, 0);
    smp::start(1, entry, 0);
}
