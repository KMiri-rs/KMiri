//@compile-flags: -Zkmiri-toml=dev/test_utils/uart16550.toml
//! A pointer to device memory built by a constant initializer has no provenance, yet device
//! memory may be accessed through it: the provenance of such a pointer is irrelevant.

use std::ptr::NonNull;

const UART_BASE: usize = 0x100_0000;
const THR: usize = 0;
const LSR: usize = 5;

struct Uart {
    base: NonNull<u8>,
}

unsafe impl Sync for Uart {}

static UART: Uart = Uart { base: NonNull::new(UART_BASE as *mut u8).unwrap() };

/// An integer that equals a device address still reads as an integer.
static UART_ADDR: usize = UART_BASE;

fn main() {
    assert_eq!(UART_ADDR + LSR, UART_BASE + 5);
    let base = UART.base.as_ptr();
    for byte in b"static ok\n" {
        unsafe {
            while base.add(LSR).read_volatile() & 0x20 == 0 {}
            base.add(THR).write_volatile(*byte);
        }
    }
}
