//! A core-local interruptor (CLINT, the SiFive layout that QEMU's `virt`
//! machine also uses) at the address that `[clint]` in the KMiri config gives,
//! reached through volatile loads and stores.
//!
//! It has one machine timer for all CPUs and, for each CPU `h`, a software
//! interrupt bit and a timer compare register:
//!
//! - `msip[h]` (4 bytes at `base + 4 * h`): bit 0 is the CPU's software
//!   interrupt, pending while it is 1.
//! - `mtimecmp[h]` (8 bytes at `base + 0x4000 + 8 * h`): the CPU's timer
//!   interrupt is pending while `mtime >= mtimecmp[h]`.
//! - `mtime` (8 bytes at `base + 0xbff8`).
//!
//! Both interrupts are levels, as on hardware: a handler that does not clear
//! the condition (write `msip` back to 0, or move `mtimecmp` past `mtime`) is
//! entered again as soon as it returns. The software interrupt is delivered as
//! vector 3 and the timer interrupt as vector 7, their RISC-V `mcause` codes;
//! the software interrupt goes first when both are pending, as on RISC-V.
//!
//! `mtime` is virtual: it advances by one every `basic_blocks_per_tick` basic
//! blocks that any CPU executes. When every CPU waits for an interrupt, time
//! jumps to the earliest `mtimecmp` of a waiting CPU; if there is none, nothing
//! can wake them and the program is reported as halted. `mtimecmp` starts at
//! `u64::MAX`, so no timer interrupt is pending until the program sets one
//! (the specification leaves the reset value unspecified).
//!
//! The model has no counterpart of the `mie` register: once interrupts are on
//! (`kern_miri_irq_enable`), both kinds are taken. Writing `msip` is not a
//! synchronization: the CPU that takes the interrupt sees the writer's earlier
//! stores only through the program's own synchronization.

use rustc_middle::ty;
use serde::Deserialize;

use crate::*;

/// `[clint]` in the KMiri config.
#[derive(Clone, Debug, Deserialize)]
pub struct ClintConfigToml {
    /// Physical address of `msip[0]`.
    pub base_addr: u64,
    /// Basic blocks per `mtime` tick.
    #[serde(default = "default_basic_blocks_per_tick")]
    pub basic_blocks_per_tick: u64,
}

fn default_basic_blocks_per_tick() -> u64 {
    1
}

/// The size of the register window.
pub const CLINT_WINDOW: u64 = 0xc000;
const MTIMECMP: u64 = 0x4000;
const MTIME: u64 = 0xbff8;
/// RISC-V `mcause` code of the machine software interrupt.
pub const CLINT_SOFTWARE_VECTOR: u8 = 3;
/// RISC-V `mcause` code of the machine timer interrupt.
pub const CLINT_TIMER_VECTOR: u8 = 7;

enum Register {
    Msip(usize),
    Mtimecmp(usize),
    Mtime,
}

pub struct Clint {
    base_addr: u64,
    basic_blocks_per_tick: u64,
    /// `mtime` minus the time that basic blocks account for.
    mtime_offset: u64,
    msip: Vec<bool>,
    mtimecmp: Vec<u64>,
}

impl Clint {
    pub fn new(config: &ClintConfigToml, num_cpus: usize) -> Self {
        Self {
            base_addr: config.base_addr,
            basic_blocks_per_tick: config.basic_blocks_per_tick,
            mtime_offset: 0,
            msip: vec![false; num_cpus],
            mtimecmp: vec![u64::MAX; num_cpus],
        }
    }

    pub fn mtime(&self, basic_block_count: u64) -> u64 {
        self.mtime_offset.wrapping_add(basic_block_count / self.basic_blocks_per_tick)
    }

    /// The interrupt of CPU `cpu` that is pending, the software one first.
    pub fn pending_vector(&self, cpu: usize, basic_block_count: u64) -> Option<u8> {
        if self.msip[cpu] {
            Some(CLINT_SOFTWARE_VECTOR)
        } else if self.mtime(basic_block_count) >= self.mtimecmp[cpu] {
            Some(CLINT_TIMER_VECTOR)
        } else {
            None
        }
    }

    /// Advances `mtime` to the earliest compare value of the CPUs in `cpus`
    /// that lies in the future, and returns whether there was one.
    pub fn skip_to_next_timer(&mut self, cpus: &[usize], basic_block_count: u64) -> bool {
        let now = self.mtime(basic_block_count);
        let Some(next) = cpus.iter().map(|&cpu| self.mtimecmp[cpu]).filter(|&t| t > now).min()
        else {
            return false;
        };
        if next == u64::MAX {
            return false;
        }
        self.mtime_offset = self.mtime_offset.wrapping_add(next - now);
        true
    }

    /// The register at `addr` if an access of `size` bytes there lies in the
    /// window. An access that overlaps the window without being exactly one
    /// register is reported.
    fn register<'tcx>(&self, addr: u64, size: u64) -> InterpResult<'tcx, Option<Register>> {
        let end = addr.saturating_add(size);
        if end <= self.base_addr || addr >= self.base_addr + CLINT_WINDOW {
            return interp_ok(None);
        }
        let offset = addr.wrapping_sub(self.base_addr);
        let num_cpus = self.msip.len() as u64;
        let register = if addr < self.base_addr {
            None
        } else if offset < MTIMECMP {
            (size == 4 && offset.is_multiple_of(4) && offset / 4 < num_cpus)
                .then(|| Register::Msip((offset / 4) as usize))
        } else if offset == MTIME {
            (size == 8).then_some(Register::Mtime)
        } else {
            let index = (offset - MTIMECMP) / 8;
            (size == 8 && offset.is_multiple_of(8) && index < num_cpus)
                .then(|| Register::Mtimecmp(index as usize))
        };
        let Some(register) = register else {
            throw_unsup_format!(
                "CLINT at {:#x}: a {size}-byte access at {addr:#x} is not one of its registers \
                 (4-byte `msip`, 8-byte `mtimecmp` and `mtime`, for {num_cpus} CPUs)",
                self.base_addr
            );
        };
        interp_ok(Some(register))
    }
}

/// Performs the volatile intrinsic `instance` if its pointer lands in the
/// register window of the configured CLINT, and returns whether it did.
pub fn volatile_on_clint<'tcx>(
    this: &mut MiriInterpCx<'tcx>,
    instance: ty::Instance<'tcx>,
    args: &[OpTy<'tcx>],
    dest: &PlaceTy<'tcx>,
) -> InterpResult<'tcx, bool> {
    let Some(clint) = &this.machine.clint else { return interp_ok(false) };
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
    let Some(register) = clint.register(addr, size.bytes())? else { return interp_ok(false) };
    let ty = match value {
        None => dest.layout.ty,
        Some(val) => val.layout.ty,
    };
    if !ty.is_integral() {
        throw_unsup_format!(
            "CLINT at {:#x}: a volatile access of type `{ty}` at {addr:#x}; only integer accesses are modelled",
            clint.base_addr
        );
    }
    let now = this.machine.basic_block_count;
    match value {
        None => {
            let clint = this.machine.clint.as_ref().unwrap();
            let value = match register {
                Register::Msip(cpu) => u64::from(clint.msip[cpu]),
                Register::Mtimecmp(cpu) => clint.mtimecmp[cpu],
                Register::Mtime => clint.mtime(now),
            };
            this.write_scalar(Scalar::from_uint(value, size), dest)?;
        }
        Some(val) => {
            let value = this.read_scalar(val)?.to_uint(size)?;
            let value = u64::try_from(value).unwrap();
            let clint = this.machine.clint.as_mut().unwrap();
            let woken = match register {
                Register::Msip(cpu) => {
                    clint.msip[cpu] = value & 1 != 0;
                    clint.msip[cpu].then_some(cpu)
                }
                Register::Mtimecmp(cpu) => {
                    clint.mtimecmp[cpu] = value;
                    None
                }
                Register::Mtime => {
                    clint.mtime_offset = value.wrapping_sub(now / clint.basic_blocks_per_tick);
                    None
                }
            };
            if let Some(cpu) = woken {
                this.machine.threads.set_cpu_halted(cpu, false);
            }
        }
    }
    interp_ok(true)
}
