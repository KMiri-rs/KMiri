//! KMiri: the untyped ("lenient") shim signature check.
//!
//! Upstream Miri removed `check_shim_sig_lenient` after moving all of its shims to the typed
//! `check_shim_sig`. KMiri's own `kern_miri_*` shims still use this check; it only verifies the
//! calling convention, that the caller is not variadic, and the argument count.

use rustc_abi::CanonAbi;
use rustc_middle::ty::Ty;
use rustc_span::Symbol;
use rustc_target::callconv::FnAbi;

use crate::*;

impl<'tcx> EvalContextExt<'tcx> for crate::MiriInterpCx<'tcx> {}
pub trait EvalContextExt<'tcx>: crate::MiriInterpCxExt<'tcx> {
    fn check_shim_sig_lenient<'a, const N: usize>(
        &mut self,
        abi: &FnAbi<'tcx, Ty<'tcx>>,
        exp_abi: CanonAbi,
        link_name: Symbol,
        args: &'a [OpTy<'tcx>],
    ) -> InterpResult<'tcx, &'a [OpTy<'tcx>; N]> {
        let ecx = self.eval_context_mut();
        ecx.check_shim_symbol_clash(link_name)?;

        if abi.conv != exp_abi {
            throw_ub_format!(
                r#"calling a function with calling convention "{exp_abi}" using caller calling convention "{}""#,
                abi.conv
            );
        }
        if abi.c_variadic {
            throw_ub_format!(
                "calling a non-variadic function with a variadic caller-side signature"
            );
        }

        if let Ok(ops) = args.try_into() {
            return interp_ok(ops);
        }
        throw_ub_format!(
            "incorrect number of arguments for `{link_name}`: got {}, expected {}",
            args.len(),
            N
        )
    }
}
