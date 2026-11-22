//! Unwinding runtime for Miri.
//!
//! The core pieces of the runtime are:
//! - An implementation of `catch_unwind` that pushes the invoked stack frame with
//!   some extra metadata derived from the panic-catching arguments of `catch_unwind`.
//! - A hack in `libpanic_unwind` that calls the `miri_start_unwind` intrinsic instead of the
//!   target-native panic runtime. (This lives in the rustc repo.)
//! - An implementation of `miri_start_unwind` that stores its argument (the panic payload), and
//!   then immediately returns, but on the *unwind* edge (not the normal return edge), thus
//!   initiating unwinding.
//! - A hook executed each time a frame is popped, such that if the frame pushed by `catch_unwind`
//!   gets popped *during unwinding*, we take the panic payload and store it according to the extra
//!   metadata we remembered when pushing said frame.

use rustc_abi::{CanonAbi, ExternAbi};
use rustc_middle::ty::Ty;
use rustc_middle::{mir, ty};
use rustc_span::Symbol;
use rustc_target::callconv::FnAbi;
use rustc_target::spec::PanicStrategy;

use crate::*;

/// Holds all of the relevant data for when unwinding hits a `try` frame.
#[derive(Debug)]
pub struct CatchUnwindData<'tcx> {
    /// The `catch_fn` callback to call in case of a panic.
    catch_fn: Pointer,
    /// The `data` argument for that callback.
    data: ImmTy<'tcx>,
    /// The return place from the original call to `try`.
    dest: MPlaceTy<'tcx>,
    /// The return block from the original call to `try`.
    ret: Option<mir::BasicBlock>,
}

impl VisitProvenance for CatchUnwindData<'_> {
    fn visit_provenance(&self, visit: &mut VisitWith<'_>) {
        let CatchUnwindData { catch_fn, data, dest, ret: _ } = self;
        catch_fn.visit_provenance(visit);
        data.visit_provenance(visit);
        dest.visit_provenance(visit);
    }
}

impl<'tcx> EvalContextExt<'tcx> for crate::MiriInterpCx<'tcx> {}
pub trait EvalContextExt<'tcx>: crate::MiriInterpCxExt<'tcx> {
    /// Handles the special `miri_start_unwind` intrinsic, which is called
    /// by libpanic_unwind to delegate the actual unwinding process to Miri.
    fn handle_miri_start_unwind(&mut self, payload: &OpTy<'tcx>) -> InterpResult<'tcx> {
        let ecx = self.eval_context_mut();

        trace!("miri_start_unwind: {:?}", ecx.frame().instance());

        let payload = ecx.read_immediate(payload)?;
        let thread = ecx.active_thread_mut();
        thread.unwind_payloads.push(payload);

        interp_ok(())
    }

    /// Handles the `catch_unwind` intrinsic.
    fn handle_catch_unwind(
        &mut self,
        try_fn: &OpTy<'tcx>,
        data: &OpTy<'tcx>,
        catch_fn: &OpTy<'tcx>,
        dest: &PlaceTy<'tcx>,
        ret: Option<mir::BasicBlock>,
    ) -> InterpResult<'tcx> {
        let ecx = self.eval_context_mut();

        // Signature:
        //   fn catch_unwind(try_fn: unsafe fn(*mut Data), data: *mut Data, catch_fn: unsafe fn(*mut Data, *mut u8)) -> bool
        // Calls `try_fn` with `data` as argument. If that executes normally, returns 0.
        // If that unwinds, calls `catch_fn` with the first argument being `data` and
        // then second argument being a target-dependent `payload` (i.e. it is up to us to define
        // what that is), and returns 1.
        // The `payload` is passed (by libstd) to `__rust_panic_cleanup`, which is then expected to
        // return a `Box<dyn Any + Send + 'static>`.
        // In Miri, `miri_start_unwind` is passed exactly that type, so we make the `payload` simply
        // a pointer to `Box<dyn Any + Send + 'static>`.

        // Get all the arguments.
        let try_fn = ecx.read_pointer(try_fn)?;
        let data = ecx.read_immediate(data)?;
        let catch_fn = ecx.read_pointer(catch_fn)?;
        let dest = ecx.force_allocation(dest)?; // needs to be valid across fn calls

        // Now we make a function call, and pass `data` as first and only argument.
        let f_instance = ecx.get_ptr_fn(try_fn)?.as_instance()?;
        trace!("[handle_catch_unwind] try_fn={f_instance:#?} ret={ret:?}");
        #[allow(clippy::cloned_ref_to_slice_refs)] // the code is clearer as-is
        ecx.call_function(
            f_instance,
            ExternAbi::Rust,
            &[data.clone()],
            None,
            // Directly return to caller.
            ReturnContinuation::Goto { ret, unwind: mir::UnwindAction::Continue },
        )?;

        // We ourselves will return `0`, eventually (will be overwritten if we catch a panic).
        ecx.write_int(0, &dest)?;

        // In unwind mode, we tag this frame with the extra data needed to catch unwinding.
        // This lets `handle_stack_pop` (below) know that we should stop unwinding
        // when we pop this frame.
        if ecx.tcx.sess.panic_strategy() == PanicStrategy::Unwind {
            ecx.frame_mut().extra.catch_unwind =
                Some(CatchUnwindData { catch_fn, data, dest, ret });
        }

        interp_ok(())
    }

    /// KMiri: is `instance` the `unwinding` crate's `_Unwind_RaiseException`?
    fn is_kmiri_raise_exception(
        &mut self,
        instance: ty::Instance<'tcx>,
    ) -> InterpResult<'tcx, bool> {
        let this = self.eval_context_mut();
        let def_id = match this.machine.kmiri_raise_exception {
            Some(def_id) => def_id,
            None => {
                let def_id = this
                    .lookup_exported_symbol(Symbol::intern("_Unwind_RaiseException"))?
                    .map(|instance| instance.def_id());
                this.machine.kmiri_raise_exception = Some(def_id);
                def_id
            }
        };
        interp_ok(def_id == Some(instance.def_id()))
    }

    /// KMiri: `_Unwind_RaiseException(exception: *mut UnwindException) -> UnwindReasonCode`
    /// of the `unwinding` crate (OSTD's `begin_panic` ends here).
    ///
    /// Natively this is a two-phase unwinder. Phase 1 walks up the stack asking each frame's
    /// personality routine for a handler and, if there is none, returns `END_OF_STACK` without
    /// unwinding anything. Phase 2 unwinds to the handler and passes `exception` to its landing
    /// pad. Here phase 1 walks Miri's stack and phase 2 is Miri's own unwinding, so cleanup
    /// blocks, `catch_unwind` and the "unwinding past a frame that does not allow unwinding"
    /// check are Miri's. The Rust code around this call (wrapping the payload, the class check
    /// in `catch_unwind`'s `do_catch`) runs unchanged.
    ///
    /// Handlers in phase 1 are what the personality routine reports as `HANDLER_FOUND`:
    /// - a frame pushed by `catch_unwind` (a catch landing pad);
    /// - a call site whose unwind action is `Terminate` (rustc emits a filter landing pad,
    ///   which phase 1 also reports as a handler; phase 2 then aborts there).
    /// A call site whose unwind action is `Unreachable` (callee declared not to unwind) is
    /// not a handler, so the search goes on past it. Unwinding through such a frame is
    /// language UB; natively phase 1 may stop there or pass through depending on how the
    /// frame was compiled. If a handler exists further up, Miri reports that UB when phase 2
    /// reaches the frame.
    ///
    /// Not modelled: frames KMiri pushes to deliver an interrupt are walked through like
    /// ordinary calls, so a `begin_panic` in an interrupt handler could find a `catch_unwind`
    /// in the interrupted code; natively the unwinder would have to cross the trap frame.
    fn kmiri_raise_exception(
        &mut self,
        abi: &FnAbi<'tcx, Ty<'tcx>>,
        args: &[OpTy<'tcx>],
        dest: &PlaceTy<'tcx>,
        ret: Option<mir::BasicBlock>,
        unwind: mir::UnwindAction,
    ) -> InterpResult<'tcx> {
        let this = self.eval_context_mut();
        // Not `check_shim_sig_lenient`: that rejects symbols the program defines itself, and
        // taking over the program's own definition is the point here.
        if abi.conv != CanonAbi::C {
            throw_ub_format!(
                "`_Unwind_RaiseException` called with calling convention {:?}",
                abi.conv
            );
        }
        let [exception] = args else {
            throw_ub_format!(
                "`_Unwind_RaiseException` called with {} arguments, expected 1",
                args.len()
            );
        };

        // Phase 1. `action` is how the call site in `frame` handles an unwind coming out of
        // the callee; it applies before `frame` itself is popped.
        let mut action = unwind;
        let mut handler_found = false;
        for frame in this.active_thread_stack().iter().rev() {
            if matches!(action, mir::UnwindAction::Terminate(_))
                || frame.extra.catch_unwind.is_some()
            {
                handler_found = true;
                break;
            }
            match frame.return_cont() {
                ReturnContinuation::Goto { unwind, .. } => action = unwind,
                ReturnContinuation::Stop { .. } => break,
            }
        }

        if handler_found {
            // Phase 2: the payload `catch_unwind` passes to `catch_fn` is the exception
            // pointer, as on a native landing pad.
            this.handle_miri_start_unwind(exception)?;
            this.unwind_to_block(unwind)
        } else {
            const END_OF_STACK: i128 = 5; // `UnwindReasonCode::END_OF_STACK`
            this.write_scalar(Scalar::from_int(END_OF_STACK, dest.layout.size), dest)?;
            this.return_to_block(ret)
        }
    }

    fn handle_stack_pop_unwind(
        &mut self,
        mut extra: FrameExtra<'tcx>,
        unwinding: bool,
    ) -> InterpResult<'tcx, ReturnAction> {
        let ecx = self.eval_context_mut();
        trace!("handle_stack_pop_unwind(extra = {:?}, unwinding = {})", extra, unwinding);

        // We only care about `catch_panic` if we're unwinding - if we're doing a normal
        // return, then we don't need to do anything special.
        if let (true, Some(catch_unwind)) = (unwinding, extra.catch_unwind.take()) {
            // We've just popped a frame that was pushed by `catch_unwind`,
            // and we are unwinding, so we should catch that.
            trace!(
                "unwinding: found catch_panic frame during unwinding: {:?}",
                ecx.frame().instance()
            );

            // We set the return value of `catch_unwind` to 1, since there was a panic.
            ecx.write_scalar(Scalar::from_bool(true), &catch_unwind.dest)?;

            // The Thread's `panic_payload` holds what was passed to `miri_start_unwind`.
            // This is exactly the second argument we need to pass to `catch_fn`.
            let payload = ecx.active_thread_mut().unwind_payloads.pop().unwrap();

            // Push the `catch_fn` stackframe.
            let f_instance = ecx.get_ptr_fn(catch_unwind.catch_fn)?.as_instance()?;
            trace!("catch_fn: {:#?}", f_instance);
            ecx.call_function(
                f_instance,
                ExternAbi::Rust,
                &[catch_unwind.data, payload],
                None,
                // Directly return to caller of `catch_unwind`.
                ReturnContinuation::Goto {
                    ret: catch_unwind.ret,
                    // `catch_fn` must not unwind.
                    unwind: mir::UnwindAction::Unreachable,
                },
            )?;

            // We pushed a new stack frame, the engine should not do any jumping now!
            interp_ok(ReturnAction::NoJump)
        } else {
            interp_ok(ReturnAction::Normal)
        }
    }
}
