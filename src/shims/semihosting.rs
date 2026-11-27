//! Host side of the Arm semihosting interface (also used on RISC-V): a
//! program on a bare machine traps with an operation number and a parameter
//! (often a pointer to a block of words) and the host performs the
//! operation. Under KMiri the trap is the `kern_miri_semihosting(op, param)`
//! call, which returns what the host would put in the result register.
//!
//! Only the console, clock and exit operations are provided. Files other
//! than the `:tt` console are not.

use std::io::Write;

use rustc_abi::Size;

use crate::*;

const SYS_OPEN: u64 = 0x01;
const SYS_CLOSE: u64 = 0x02;
const SYS_WRITEC: u64 = 0x03;
const SYS_WRITE0: u64 = 0x04;
const SYS_WRITE: u64 = 0x05;
const SYS_ISTTY: u64 = 0x09;
const SYS_CLOCK: u64 = 0x10;
const SYS_ERRNO: u64 = 0x13;
const SYS_EXIT: u64 = 0x18;
const SYS_EXIT_EXTENDED: u64 = 0x20;

/// `ADP_Stopped_ApplicationExit`: a normal exit; the sub-code is the status.
const APPLICATION_EXIT: u64 = 0x20026;

// Console handles returned by opening `:tt`.
const STDIN: u64 = 1;
const STDOUT: u64 = 2;
const STDERR: u64 = 3;

impl<'tcx> EvalContextExt<'tcx> for crate::MiriInterpCx<'tcx> {}
pub trait EvalContextExt<'tcx>: crate::MiriInterpCxExt<'tcx> {
    /// Performs semihosting operation `op` with parameter `param`; returns the
    /// value of the result register.
    fn semihosting_call(&mut self, op: u64, param: Pointer) -> InterpResult<'tcx, u64> {
        let this = self.eval_context_mut();
        let word = this.pointer_size();
        let field = |this: &mut MiriInterpCx<'tcx>, index: u64| -> InterpResult<'tcx, Scalar> {
            let ptr = param.wrapping_offset(word * index, this);
            // Read as a pointer: a field may hold one, and reading it as an
            // integer would drop its provenance.
            let place = this.ptr_to_mplace(ptr, this.machine.layouts.unit_ptr_mut);
            this.read_scalar(&place)
        };
        let out = |handle: u64, bytes: &[u8]| {
            let _ignore = match handle {
                STDERR => std::io::stderr().write_all(bytes),
                _ => std::io::stdout().write_all(bytes),
            };
        };
        interp_ok(match op {
            // Block: name, mode, name length. Only the console exists; its
            // mode picks the stream (r: stdin, w: stdout, a: stderr).
            SYS_OPEN => {
                let name = field(this, 0)?.to_pointer(this);
                let mode = field(this, 1)?.to_target_usize(this)?;
                let len = field(this, 2)?.to_target_usize(this)?;
                let name = this.read_bytes_ptr_strip_provenance(name, Size::from_bytes(len))?;
                if name != b":tt" {
                    throw_unsup_format!("semihosting: only the `:tt` console can be opened");
                }
                match mode {
                    0..=3 => STDIN,
                    4..=7 => STDOUT,
                    _ => STDERR,
                }
            }
            SYS_CLOSE | SYS_ERRNO => 0,
            SYS_ISTTY => 1,
            SYS_WRITEC => {
                let byte = this.read_bytes_ptr_strip_provenance(param, Size::from_bytes(1))?;
                out(STDOUT, byte);
                0
            }
            SYS_WRITE0 => {
                let s = this.read_c_str(param)?.to_vec();
                out(STDOUT, &s);
                0
            }
            // Block: handle, buffer, length. Returns the number of bytes not
            // written.
            SYS_WRITE => {
                let handle = field(this, 0)?.to_target_usize(this)?;
                let buf = field(this, 1)?.to_pointer(this);
                let len = field(this, 2)?.to_target_usize(this)?;
                let bytes =
                    this.read_bytes_ptr_strip_provenance(buf, Size::from_bytes(len))?.to_vec();
                out(handle, &bytes);
                0
            }
            // Centiseconds since the program started, on the interpreter's
            // monotonic clock.
            SYS_CLOCK => {
                let clock = &this.machine.monotonic_clock;
                let elapsed = clock.now().duration_since(clock.epoch());
                u64::try_from(elapsed.as_millis() / 10).unwrap_or(u64::MAX)
            }
            // On 64-bit targets both take a block: reason, sub-code.
            SYS_EXIT | SYS_EXIT_EXTENDED => {
                let reason = field(this, 0)?.to_target_usize(this)?;
                let code = field(this, 1)?.to_target_usize(this)?;
                let exit_code =
                    if reason == APPLICATION_EXIT { i32::try_from(code).unwrap_or(1) } else { 1 };
                throw_machine_stop!(TerminationInfo::Exit { exit_code, leak_check: false });
            }
            _ => throw_unsup_format!("semihosting operation {op:#x} is not supported"),
        })
    }
}
