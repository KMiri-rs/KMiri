//@compile-flags: -Zkmiri-toml=dev/test_utils/uart16550.toml -Zmiri-permissive-provenance
// A driver of the modelled 16550 UART: it polls the line status before each
// byte, and the bytes it sends reach standard output.

use std::ptr::{read_volatile, write_volatile};

const BASE: usize = 0x100_0000;
const THR: usize = 0;
const IIR_FCR: usize = 2;
const LCR: usize = 3;
const LSR: usize = 5;
const SCR: usize = 7;
const LSR_THRE: u8 = 0x20;

fn reg(n: usize) -> *mut u8 {
    (BASE + n) as *mut u8
}

fn main() {
    unsafe {
        // Divisor latch: DLL/DLM shadow THR/IER while LCR.DLAB is set.
        write_volatile(reg(LCR), 0x80);
        write_volatile(reg(THR), 0x01);
        assert_eq!(read_volatile(reg(THR)), 0x01);
        write_volatile(reg(LCR), 0x03);
        assert_eq!(read_volatile(reg(LCR)), 0x03);
        // No interrupt pending, FIFO bits once enabled.
        assert_eq!(read_volatile(reg(IIR_FCR)), 0x01);
        write_volatile(reg(IIR_FCR), 0x07);
        assert_eq!(read_volatile(reg(IIR_FCR)), 0xc1);
        write_volatile(reg(SCR), 0x5a);
        assert_eq!(read_volatile(reg(SCR)), 0x5a);
        for &byte in b"uart ok\n" {
            while read_volatile(reg(LSR)) & LSR_THRE == 0 {}
            write_volatile(reg(THR), byte);
        }
    }
}
