//! A 16550-compatible UART at the address that `[[uart16550]]` in the KMiri
//! config gives, reached through volatile loads and stores.
//!
//! Without an MMU a kernel reaches device registers by plain volatile
//! accesses to their physical address. The memory under the register window is
//! an ordinary allocation (a `[[kalloc]]` entry), so references to the
//! register block stay valid; the volatile accesses inside the window go to
//! this model instead of that memory.
//!
//! The model is a console that is always ready to send and never receives:
//! a byte written to the transmit holding register goes to standard output at
//! once, the line status register always reads "transmitter empty" and
//! "no data", and no interrupt is ever pending (IIR reads 0x01, plus the FIFO
//! bits once FCR enabled them). The other registers read back what was last
//! written to them. The model raises no interrupt, so a driver that waits for
//! a transmit or receive interrupt waits forever.
//!
//! Register `n` is at offset `n << reg_shift`; an access may be up to
//! `1 << reg_shift` bytes wide and uses its low 8 bits.

use std::io::Write;

use rustc_middle::ty;
use serde::Deserialize;

use crate::*;

/// `[[uart16550]]` in the KMiri config.
#[derive(Clone, Debug, Deserialize)]
pub struct Uart16550ConfigToml {
    /// Physical address of register 0.
    pub base_addr: u64,
    /// Register `n` is at `base_addr + (n << reg_shift)`.
    #[serde(default)]
    pub reg_shift: u32,
}

const LCR_DLAB: u8 = 0x80;
const FCR_FIFO_ENABLE: u8 = 0x01;
const IIR_NO_INTERRUPT: u8 = 0x01;
const IIR_FIFOS_ENABLED: u8 = 0xc0;
/// Transmit holding register empty and transmitter empty; no data ready.
const LSR_IDLE: u8 = 0x60;

pub struct Uart16550 {
    base_addr: u64,
    reg_shift: u32,
    ier: u8,
    fcr: u8,
    lcr: u8,
    mcr: u8,
    scr: u8,
    dll: u8,
    dlm: u8,
}

impl Uart16550 {
    pub fn new(config: &Uart16550ConfigToml) -> Self {
        Self {
            base_addr: config.base_addr,
            reg_shift: config.reg_shift,
            ier: 0,
            fcr: 0,
            lcr: 0,
            mcr: 0,
            scr: 0,
            dll: 0,
            dlm: 0,
        }
    }

    fn window(&self) -> u64 {
        8 << self.reg_shift
    }

    /// The register at `addr` if an access of `size` bytes there lies in the
    /// window. An access that overlaps the window without fitting one register
    /// is reported.
    fn register<'tcx>(&self, addr: u64, size: u64) -> InterpResult<'tcx, Option<u64>> {
        let end = addr.saturating_add(size);
        if end <= self.base_addr || addr >= self.base_addr + self.window() {
            return interp_ok(None);
        }
        let offset = addr.wrapping_sub(self.base_addr);
        let stride = 1u64 << self.reg_shift;
        if addr < self.base_addr || !offset.is_multiple_of(stride) || size > stride {
            throw_unsup_format!(
                "16550 UART at {:#x}: a {size}-byte access at {addr:#x} does not fit one register",
                self.base_addr
            );
        }
        interp_ok(Some(offset >> self.reg_shift))
    }

    fn read(&self, reg: u64) -> u8 {
        let dlab = self.lcr & LCR_DLAB != 0;
        match reg {
            0 if dlab => self.dll,
            0 => 0,
            1 if dlab => self.dlm,
            1 => self.ier,
            2 =>
                IIR_NO_INTERRUPT
                    | if self.fcr & FCR_FIFO_ENABLE != 0 { IIR_FIFOS_ENABLED } else { 0 },
            3 => self.lcr,
            4 => self.mcr,
            5 => LSR_IDLE,
            6 => 0,
            7 => self.scr,
            _ => unreachable!(),
        }
    }

    fn write(&mut self, reg: u64, value: u8) {
        let dlab = self.lcr & LCR_DLAB != 0;
        match reg {
            0 if dlab => self.dll = value,
            0 => {
                let mut stdout = std::io::stdout();
                let _ignore = stdout.write_all(&[value]);
                let _ignore = stdout.flush();
            }
            1 if dlab => self.dlm = value,
            1 => self.ier = value,
            2 => self.fcr = value,
            3 => self.lcr = value,
            4 => self.mcr = value,
            5 | 6 => {} // read-only
            7 => self.scr = value,
            _ => unreachable!(),
        }
    }
}

/// Performs the volatile intrinsic `instance` if its pointer lands in the
/// register window of a configured 16550 UART, and returns whether it did.
pub fn volatile_on_uart16550<'tcx>(
    this: &mut MiriInterpCx<'tcx>,
    instance: ty::Instance<'tcx>,
    args: &[OpTy<'tcx>],
    dest: &PlaceTy<'tcx>,
) -> InterpResult<'tcx, bool> {
    if this.machine.uart16550.borrow().is_empty() {
        return interp_ok(false);
    }
    let name = this.tcx.item_name(instance.def_id());
    let (ptr, size, value) = match name.as_str() {
        "volatile_load" | "unaligned_volatile_load" => {
            let [ptr] = args else { return interp_ok(false) };
            (this.read_pointer(ptr)?, dest.layout.size, None)
        }
        "volatile_store" | "unaligned_volatile_store" => {
            let [ptr, val] = args else { return interp_ok(false) };
            (this.read_pointer(ptr)?, val.layout.size, Some(val))
        }
        _ => return interp_ok(false),
    };
    let addr = ptr.addr().bytes();
    let mut found = None;
    for (index, uart) in this.machine.uart16550.borrow().iter().enumerate() {
        if let Some(reg) = uart.register(addr, size.bytes())? {
            found = Some((index, reg));
            break;
        }
    }
    let Some((index, reg)) = found else { return interp_ok(false) };
    let ty = match value {
        None => dest.layout.ty,
        Some(val) => val.layout.ty,
    };
    if !ty.is_integral() {
        throw_unsup_format!(
            "16550 UART at {:#x}: a volatile access of type `{ty}` at {addr:#x}; only integer accesses are modelled",
            this.machine.uart16550.borrow()[index].base_addr
        );
    }
    match value {
        None => {
            let byte = this.machine.uart16550.borrow()[index].read(reg);
            this.write_scalar(Scalar::from_uint(byte, size), dest)?;
        }
        Some(val) => {
            let byte = this.read_scalar(val)?.to_uint(size)? as u8;
            this.machine.uart16550.borrow_mut()[index].write(reg, byte);
        }
    }
    interp_ok(true)
}
