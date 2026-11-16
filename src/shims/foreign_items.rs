#![allow(clippy::arithmetic_side_effects)]
use std::collections::hash_map::Entry;
use std::io::Write;
use std::path::Path;

use rustc_abi::{Align, CanonAbi, ExternAbi, Size};
use rustc_ast::expand::allocator::NO_ALLOC_SHIM_IS_UNSTABLE;
use rustc_data_structures::either::Either;
use rustc_hir::attrs::Linkage;
use rustc_hir::def::DefKind;
use rustc_hir::def_id::CrateNum;
use rustc_middle::middle::codegen_fn_attrs::CodegenFnAttrFlags;
use rustc_middle::mir::interpret::AllocInit;
use rustc_middle::ty::{Instance, Ty};
use rustc_middle::{mir, ty};
use rustc_span::Symbol;
use rustc_target::callconv::FnAbi;
use rustc_target::spec::Os;

use super::alloc::EvalContextExt as _;
use super::backtrace::EvalContextExt as _;
use crate::concurrency::GenmcEvalContextExt as _;
use crate::concurrency::thread::Stack;
use crate::helpers::EvalContextExt as _;
use crate::mirch::{
    PageState, PageTable, TypedKind, dealloc_pages, insert_init_mask, type_pages_at,
};
use crate::*;

/// Type of dynamic symbols (for `dlsym` et al)
#[derive(Debug, Copy, Clone)]
pub struct DynSym(Symbol);

#[expect(clippy::should_implement_trait)]
impl DynSym {
    pub fn from_str(name: &str) -> Self {
        DynSym(Symbol::intern(name))
    }
}

impl<'tcx> EvalContextExt<'tcx> for crate::MiriInterpCx<'tcx> {}
pub trait EvalContextExt<'tcx>: crate::MiriInterpCxExt<'tcx> {
    /// Emulates calling a foreign item, failing if the item is not supported.
    /// This function will handle `goto_block` if needed.
    /// Returns Ok(None) if the foreign item was completely handled
    /// by this function.
    /// Returns Ok(Some(body)) if processing the foreign item
    /// is delegated to another function.
    fn emulate_foreign_item(
        &mut self,
        link_name: Symbol,
        abi: &FnAbi<'tcx, Ty<'tcx>>,
        args: &[OpTy<'tcx>],
        dest: &PlaceTy<'tcx>,
        ret: Option<mir::BasicBlock>,
        unwind: mir::UnwindAction,
    ) -> InterpResult<'tcx, Option<(&'tcx mir::Body<'tcx>, ty::Instance<'tcx>)>> {
        let this = self.eval_context_mut();

        // Handle allocator shim.
        if let Some(shim) = this.machine.allocator_shim_symbols.get(&link_name) {
            match *shim {
                Either::Left(other_fn) => {
                    let handler = this
                        .lookup_exported_symbol(other_fn)?
                        .expect("missing alloc error handler symbol");
                    return interp_ok(Some(handler));
                }
                Either::Right(special) => {
                    this.rust_special_allocator_method(special, link_name, abi, args, dest)?;
                    this.return_to_block(ret)?;
                    return interp_ok(None);
                }
            }
        }

        // FIXME: avoid allocating memory
        let dest = this.force_allocation(dest)?;

        // If the user configured this symbol to prefer their own implementation,
        // skip the built-in shim entirely and delegate to the user's body directly.
        if this
            .machine
            .kmiri_toml
            .as_ref()
            .is_some_and(|c| c.prefers_user_implementation(link_name.as_str()))
        {
            if let Some(body) = this.lookup_exported_symbol(link_name)? {
                return interp_ok(Some(body));
            }
        }

        // The rest either implements the logic, or falls back to `lookup_exported_symbol`.
        let res = this.emulate_foreign_item_inner(link_name, abi, args, &dest)?;
        res.jump_to_next_block(this, &dest, ret, Some(unwind), |this| {
            if let Some(body) = this.lookup_exported_symbol(link_name)? {
                return interp_ok(Some(body));
            }

            throw_machine_stop!(TerminationInfo::UnsupportedForeignItem(format!(
                "can't call foreign function `{link_name}` on OS `{os}`",
                os = this.tcx.sess.target.os,
            )));
        })
    }

    fn is_dyn_sym(&self, name: &str) -> bool {
        let this = self.eval_context_ref();
        match &this.tcx.sess.target.os {
            os if this.target_os_is_unix() => shims::unix::foreign_items::is_dyn_sym(name, os),
            Os::Windows => shims::windows::foreign_items::is_dyn_sym(name),
            _ => false,
        }
    }

    /// Emulates a call to a `DynSym`.
    fn emulate_dyn_sym(
        &mut self,
        sym: DynSym,
        abi: &FnAbi<'tcx, Ty<'tcx>>,
        args: &[OpTy<'tcx>],
        dest: &PlaceTy<'tcx>,
        ret: Option<mir::BasicBlock>,
        unwind: mir::UnwindAction,
    ) -> InterpResult<'tcx> {
        let res = self.emulate_foreign_item(sym.0, abi, args, dest, ret, unwind)?;
        assert!(res.is_none(), "DynSyms that delegate are not supported");
        interp_ok(())
    }

    /// Lookup the body of a function that has `link_name` as the symbol name.
    fn lookup_exported_symbol(
        &mut self,
        link_name: Symbol,
    ) -> InterpResult<'tcx, Option<(&'tcx mir::Body<'tcx>, ty::Instance<'tcx>)>> {
        let this = self.eval_context_mut();
        match this.lookup_exported_symbol_instance(link_name)? {
            None => interp_ok(None), // no symbol with this name
            Some(instance) => interp_ok(Some((this.load_mir(instance.def, None)?, instance))),
        }
    }

    /// Like `lookup_exported_symbol`, but does not load the MIR. Exported statics of
    /// dependency crates have no `mir_for_ctfe`, so loading it would ICE.
    fn lookup_exported_symbol_instance(
        &mut self,
        link_name: Symbol,
    ) -> InterpResult<'tcx, Option<ty::Instance<'tcx>>> {
        let this = self.eval_context_mut();
        let tcx = this.tcx.tcx;

        // If the result was cached, just return it.
        // (Cannot use `or_insert` since the code below might have to throw an error.)
        let entry = this.machine.exported_symbols_cache.entry(link_name);
        let instance = *match entry {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => {
                // Find it if it was not cached.

                struct SymbolTarget<'tcx> {
                    instance: ty::Instance<'tcx>,
                    cnum: CrateNum,
                    is_weak: bool,
                }
                let mut symbol_target: Option<SymbolTarget<'tcx>> = None;
                helpers::iter_exported_symbols(tcx, |cnum, def_id| {
                    let attrs = tcx.codegen_fn_attrs(def_id);
                    // Skip over imports of items.
                    if tcx.is_foreign_item(def_id) {
                        return interp_ok(());
                    }
                    // Skip over items without an explicitly defined symbol name.
                    if !(attrs.symbol_name.is_some()
                        || attrs.flags.contains(CodegenFnAttrFlags::NO_MANGLE)
                        || attrs.flags.contains(CodegenFnAttrFlags::RUSTC_STD_INTERNAL_SYMBOL))
                    {
                        return interp_ok(());
                    }

                    let instance = Instance::mono(tcx, def_id);
                    let symbol_name = tcx.symbol_name(instance).name;
                    let is_weak = attrs.linkage == Some(Linkage::WeakAny);
                    if symbol_name == link_name.as_str() {
                        if let Some(original) = &symbol_target {
                            // There is more than one definition with this name. What we do now
                            // depends on whether one or both definitions are weak.
                            match (is_weak, original.is_weak) {
                                (false, true) => {
                                    // Original definition is a weak definition. Override it.

                                    symbol_target = Some(SymbolTarget {
                                        instance: ty::Instance::mono(tcx, def_id),
                                        cnum,
                                        is_weak,
                                    });
                                }
                                (true, false) => {
                                    // Current definition is a weak definition. Keep the original one.
                                }
                                (true, true) | (false, false) => {
                                    // Either both definitions are non-weak or both are weak. In
                                    // either case return an error. For weak definitions we error
                                    // because it is unspecified which definition would have been
                                    // picked by the linker.

                                    // Make sure we are consistent wrt what is 'first' and 'second'.
                                    let original_span =
                                        tcx.def_span(original.instance.def_id()).data();
                                    let span = tcx.def_span(def_id).data();
                                    if original_span < span {
                                        throw_machine_stop!(
                                            TerminationInfo::MultipleSymbolDefinitions {
                                                link_name,
                                                first: original_span,
                                                first_crate: tcx.crate_name(original.cnum),
                                                second: span,
                                                second_crate: tcx.crate_name(cnum),
                                            }
                                        );
                                    } else {
                                        throw_machine_stop!(
                                            TerminationInfo::MultipleSymbolDefinitions {
                                                link_name,
                                                first: span,
                                                first_crate: tcx.crate_name(cnum),
                                                second: original_span,
                                                second_crate: tcx.crate_name(original.cnum),
                                            }
                                        );
                                    }
                                }
                            }
                        } else {
                            symbol_target = Some(SymbolTarget {
                                instance: ty::Instance::mono(tcx, def_id),
                                cnum,
                                is_weak,
                            });
                        }
                    }
                    interp_ok(())
                })?;

                // Once we identified the instance corresponding to the symbol, ensure
                // it is a function. It is okay to encounter non-functions in the search above
                // as long as the final instance we arrive at is a function.
                if let Some(SymbolTarget { instance, .. }) = symbol_target {
                    if !matches!(
                        tcx.def_kind(instance.def_id()),
                        DefKind::Fn | DefKind::AssocFn | DefKind::Static { .. }
                    ) {
                        throw_ub_format!(
                            "attempt to call an exported symbol that is not defined as a function"
                        );
                    }
                }

                e.insert(symbol_target.map(|SymbolTarget { instance, .. }| instance))
            }
        };
        interp_ok(instance)
    }
}

impl<'tcx> EvalContextExtPriv<'tcx> for crate::MiriInterpCx<'tcx> {}
trait EvalContextExtPriv<'tcx>: crate::MiriInterpCxExt<'tcx> {
    fn emulate_foreign_item_inner(
        &mut self,
        link_name: Symbol,
        abi: &FnAbi<'tcx, Ty<'tcx>>,
        args: &[OpTy<'tcx>],
        dest: &MPlaceTy<'tcx>,
    ) -> InterpResult<'tcx, EmulateItemResult> {
        let this = self.eval_context_mut();

        // First deal with any external C functions in linked .so file.
        #[cfg(all(feature = "native-lib", unix))]
        if !this.machine.native_lib.is_empty() {
            use crate::shims::native_lib::EvalContextExt as _;
            // An Ok(false) here means that the function being called was not exported
            // by the specified `.so` file; we should continue and check if it corresponds to
            // a provided shim.
            if this.call_native_fn(link_name, dest, args)? {
                return interp_ok(EmulateItemResult::NeedsReturn);
            }
        }
        // When adding a new shim, you should follow the following pattern:
        // ```
        // "shim_name" => {
        //     let [arg1, arg2, arg3] = this.check_shim_sig_lenient(abi, CanonAbi::C , link_name, args)?;
        //     let result = this.shim_name(arg1, arg2, arg3)?;
        //     this.write_scalar(result, dest)?;
        // }
        // ```
        // and then define `shim_name` as a helper function in an extension trait in a suitable file
        // (see e.g. `unix/fs.rs`):
        // ```
        // fn shim_name(
        //     &mut self,
        //     arg1: &OpTy<'tcx>,
        //     arg2: &OpTy<'tcx>,
        //     arg3: &OpTy<'tcx>,
        //     arg4: &OpTy<'tcx>)
        // -> InterpResult<'tcx, Scalar> {
        //     let this = self.eval_context_mut();
        //
        //     // First thing: load all the arguments. Details depend on the shim.
        //     let arg1 = this.read_scalar(arg1)?.to_u32()?;
        //     let arg2 = this.read_pointer(arg2)?; // when you need to work with the pointer directly
        //     let arg3 = this.deref_pointer_as(arg3, this.libc_ty_layout("some_libc_struct"))?; // when you want to load/store
        //         // through the pointer and supply the type information yourself
        //     let arg4 = this.deref_pointer(arg4)?; // when you want to load/store through the pointer and trust
        //         // the user-given type (which you shouldn't usually do)
        //
        //     // ...
        //
        //     interp_ok(Scalar::from_u32(42))
        // }
        // ```
        // You might find existing shims not following this pattern, most
        // likely because they predate it or because for some reason they cannot be made to fit.

        // Here we dispatch all the shims for foreign functions. If you have a platform specific
        // shim, add it to the corresponding submodule.
        match link_name.as_str() {
            // Magic function Rust emits (and not as part of the allocator shim).
            name if name == this.mangle_internal_symbol(NO_ALLOC_SHIM_IS_UNSTABLE) => {
                // This is a no-op shim that only exists to prevent making the allocator shims
                // instantly stable.
                let [] = this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
            }

            // Miri-specific extern functions
            "miri_alloc" => {
                let [size, align] =
                    this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let size = this.read_target_usize(size)?;
                let align = this.read_target_usize(align)?;

                this.check_rust_alloc_request(size, align)?;

                let ptr = this.allocate_ptr(
                    Size::from_bytes(size),
                    Align::from_bytes(align).unwrap(),
                    MiriMemoryKind::Miri.into(),
                    AllocInit::Uninit,
                )?;

                this.write_pointer(ptr, dest)?;
            }
            "miri_dealloc" => {
                let [ptr, old_size, align] =
                    this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let ptr = this.read_pointer(ptr)?;
                let old_size = this.read_target_usize(old_size)?;
                let align = this.read_target_usize(align)?;

                // No need to check old_size/align; we anyway check that they match the allocation.
                this.deallocate_ptr(
                    ptr,
                    Some((Size::from_bytes(old_size), Align::from_bytes(align).unwrap())),
                    MiriMemoryKind::Miri.into(),
                )?;
            }
            "miri_track_alloc" => {
                let [ptr] = this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let ptr = this.read_pointer(ptr)?;
                let (alloc_id, _, _) = this.ptr_get_alloc_id(ptr, 0).map_err_kind(|_e| {
                    err_machine_stop!(TerminationInfo::Abort(format!(
                        "pointer passed to `miri_get_alloc_id` must not be dangling, got {ptr:?}"
                    )))
                })?;
                if this.machine.tracked_alloc_ids.insert(alloc_id) {
                    let info = this.get_alloc_info(alloc_id);
                    this.emit_diagnostic(NonHaltingDiagnostic::TrackingAlloc(
                        alloc_id, info.size, info.align,
                    ));
                }
            }
            "miri_start_unwind" => {
                let [payload] =
                    this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                this.handle_miri_start_unwind(payload)?;
                return interp_ok(EmulateItemResult::NeedsUnwind);
            }
            "miri_run_provenance_gc" => {
                let [] = this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                this.run_provenance_gc();
            }
            "miri_get_alloc_id" => {
                let [ptr] = this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let ptr = this.read_pointer(ptr)?;
                let (alloc_id, _, _) = this.ptr_get_alloc_id(ptr, 0).map_err_kind(|_e| {
                    err_machine_stop!(TerminationInfo::Abort(format!(
                        "pointer passed to `miri_get_alloc_id` must not be dangling, got {ptr:?}"
                    )))
                })?;
                this.write_scalar(Scalar::from_u64(alloc_id.0.get()), dest)?;
            }
            "miri_print_borrow_state" => {
                let [id, show_unnamed] =
                    this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let id = this.read_scalar(id)?.to_u64()?;
                let show_unnamed = this.read_scalar(show_unnamed)?.to_bool()?;
                if let Some(id) = std::num::NonZero::new(id).map(AllocId)
                    && this.get_alloc_info(id).kind == AllocKind::LiveData
                {
                    this.print_borrow_state(id, show_unnamed)?;
                } else {
                    eprintln!("{id} is not the ID of a live data allocation");
                }
            }
            "miri_pointer_name" => {
                // This associates a name to a tag. Very useful for debugging, and also makes
                // tests more strict.
                let [ptr, nth_parent, name] =
                    this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let ptr = this.read_pointer(ptr)?;
                let nth_parent = this.read_scalar(nth_parent)?.to_u8()?;
                let name = this.read_immediate(name)?;

                let name = this.read_byte_slice(&name)?;
                // We must make `name` owned because we need to
                // end the shared borrow from `read_byte_slice` before we can
                // start the mutable borrow for `give_pointer_debug_name`.
                let name = String::from_utf8_lossy(name).into_owned();
                this.give_pointer_debug_name(ptr, nth_parent, &name)?;
            }
            "miri_static_root" => {
                let [ptr] = this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let ptr = this.read_pointer(ptr)?;
                let (alloc_id, offset, _) = this.ptr_get_alloc_id(ptr, 0)?;
                if offset != Size::ZERO {
                    throw_unsup_format!(
                        "pointer passed to `miri_static_root` must point to beginning of an allocated block"
                    );
                }
                this.machine.static_roots.push(alloc_id);
            }
            "miri_host_to_target_path" => {
                let [ptr, out, out_size] =
                    this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let ptr = this.read_pointer(ptr)?;
                let out = this.read_pointer(out)?;
                let out_size = this.read_scalar(out_size)?.to_target_usize(this)?;

                // The host affects program behavior here, so this requires isolation to be disabled.
                this.check_no_isolation("`miri_host_to_target_path`")?;

                // We read this as a plain OsStr and write it as a path, which will convert it to the target.
                let path = this.read_os_str_from_c_str(ptr)?.to_owned();
                let (success, needed_size) =
                    this.write_path_to_c_str(Path::new(&path), out, out_size)?;
                // Return value: 0 on success, otherwise the size it would have needed.
                this.write_int(if success { 0 } else { needed_size }, dest)?;
            }
            "miri_thread_spawn" => {
                // FIXME: `check_shim_sig` does not work with function pointers.
                let [start_routine, func_arg] =
                    this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let start_routine = this.read_pointer(start_routine)?;
                let func_arg = this.read_immediate(func_arg)?;

                this.start_regular_thread(
                    Some(dest.clone()),
                    start_routine,
                    ExternAbi::Rust,
                    func_arg,
                    this.machine.layouts.unit,
                    None,
                )?;
            }
            "miri_thread_join" => {
                let [thread_id] = this.check_shim_sig(
                    shim_sig!(extern "Rust" fn(usize) -> bool),
                    link_name,
                    abi,
                    args,
                )?;

                let thread = this.read_target_usize(thread_id)?;
                // Joining a terminated thread is valid.
                use crate::concurrency::thread::ThreadLookupError;
                let thread = match this.thread_id_try_from(thread) {
                    Ok(id) | Err(ThreadLookupError::Terminated(id)) => Some(id),
                    Err(ThreadLookupError::InvalidId) => None,
                };
                if let Some(thread) = thread {
                    this.join_thread_exclusive(
                        thread,
                        /* success_retval */ Scalar::from_bool(true),
                        dest,
                    )?;
                } else {
                    this.write_scalar(Scalar::from_bool(false), dest)?;
                }
            }
            // Hint that a loop is spinning indefinitely.
            "miri_spin_loop" => {
                let [] = this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;

                // Try to run another thread to maximize the chance of finding actual bugs.
                this.yield_active_thread();
            }
            // Obtains the size of a Miri backtrace. See the README for details.
            "miri_backtrace_size" => {
                this.handle_miri_backtrace_size(abi, link_name, args, dest)?;
            }
            // Obtains a Miri backtrace. See the README for details.
            "miri_get_backtrace" => {
                // `check_shim` happens inside `handle_miri_get_backtrace`.
                this.handle_miri_get_backtrace(abi, link_name, args)?;
            }
            // Resolves a Miri backtrace frame. See the README for details.
            "miri_resolve_frame" => {
                // `check_shim` happens inside `handle_miri_resolve_frame`.
                this.handle_miri_resolve_frame(abi, link_name, args, dest)?;
            }
            // Writes the function and file names of a Miri backtrace frame into a user provided buffer. See the README for details.
            "miri_resolve_frame_names" => {
                this.handle_miri_resolve_frame_names(abi, link_name, args)?;
            }
            // Writes some bytes to the interpreter's stdout/stderr. See the
            // README for details.
            "miri_write_to_stdout" | "miri_write_to_stderr" => {
                let [msg] = this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let msg = this.read_immediate(msg)?;
                let msg = this.read_byte_slice(&msg)?;
                if this.machine.debugger.is_some() {
                    this.machine
                        .push_debugger_output(link_name.as_str() == "miri_write_to_stderr", msg);
                    return interp_ok(EmulateItemResult::NeedsReturn);
                }
                log!("[stdout/stderr] {}", std::str::from_utf8(msg).unwrap());
                // Note: we're ignoring errors writing to host stdout/stderr.
                let _ignore = match link_name.as_str() {
                    "miri_write_to_stdout" => std::io::stdout().write_all(msg),
                    "miri_write_to_stderr" => std::io::stderr().write_all(msg),
                    _ => unreachable!(),
                };
            }
            // Promises that a pointer has a given symbolic alignment.
            "miri_promise_symbolic_alignment" => {
                use rustc_abi::AlignFromBytesError;

                let [ptr, align] =
                    this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let ptr = this.read_pointer(ptr)?;
                let align = this.read_target_usize(align)?;
                if !align.is_power_of_two() {
                    throw_unsup_format!(
                        "`miri_promise_symbolic_alignment`: alignment must be a power of 2, got {align}"
                    );
                }
                let align = Align::from_bytes(align).unwrap_or_else(|err| {
                    match err {
                        AlignFromBytesError::NotPowerOfTwo(_) => unreachable!(),
                        // When the alignment is a power of 2 but too big, clamp it to MAX.
                        AlignFromBytesError::TooLarge(_) => Align::MAX,
                    }
                });
                let addr = ptr.addr();
                // Cannot panic since `align` is a power of 2 and hence non-zero.
                if addr.bytes().strict_rem(align.bytes()) != 0 {
                    throw_unsup_format!(
                        "`miri_promise_symbolic_alignment`: pointer is not actually aligned"
                    );
                }
                if let Ok((alloc_id, offset, ..)) = this.ptr_try_get_alloc_id(ptr, 0) {
                    let alloc_align = this.get_alloc_info(alloc_id).align;
                    // If the newly promised alignment is bigger than the native alignment of this
                    // allocation, and bigger than the previously promised alignment, then set it.
                    if align > alloc_align
                        && this
                            .machine
                            .symbolic_alignment
                            .get_mut()
                            .get(&alloc_id)
                            .is_none_or(|&(_, old_align)| align > old_align)
                    {
                        this.machine.symbolic_alignment.get_mut().insert(alloc_id, (offset, align));
                    }
                }
            }
            // GenMC mode: Assume statements block the current thread when their condition is false.
            "miri_genmc_assume" => {
                let [condition] =
                    this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                if this.machine.data_race.as_genmc_ref().is_some() {
                    this.handle_genmc_verifier_assume(condition)?;
                } else {
                    throw_unsup_format!("miri_genmc_assume is only supported in GenMC mode")
                }
            }

            // Aborting the process.
            "exit" => {
                // FIXME: This does not have a direct test (#3179).
                let [code] = this.check_shim_sig_lenient(abi, CanonAbi::C, link_name, args)?;
                let code = this.read_scalar(code)?.to_i32()?;
                if let Some(genmc_ctx) = this.machine.data_race.as_genmc_ref() {
                    // If there is no error, execution should continue (on a different thread).
                    genmc_ctx.handle_exit(
                        this.machine.threads.active_thread(),
                        code,
                        crate::concurrency::ExitType::ExitCalled,
                    )?;
                    return interp_ok(EmulateItemResult::AlreadyJumped);
                }
                throw_machine_stop!(TerminationInfo::Exit { code, leak_check: false });
            }
            "abort" => {
                // FIXME: This does not have a direct test (#3179).
                let [] = this.check_shim_sig_lenient(abi, CanonAbi::C, link_name, args)?;
                throw_machine_stop!(TerminationInfo::Abort(
                    "the program aborted execution".to_owned()
                ));
            }

            // Standard C allocation
            "malloc" => {
                let [size] = this.check_shim_sig_lenient(abi, CanonAbi::C, link_name, args)?;
                let size = this.read_target_usize(size)?;
                if size <= this.max_size_of_val().bytes() {
                    let res = this.malloc(size, AllocInit::Uninit)?;
                    this.write_pointer(res, dest)?;
                } else {
                    // If this does not fit in an isize, return null and, on Unix, set errno.
                    if this.target_os_is_unix() {
                        this.set_last_error(LibcError("ENOMEM"))?;
                    }
                    this.write_null(dest)?;
                }
            }
            "calloc" => {
                let [items, elem_size] =
                    this.check_shim_sig_lenient(abi, CanonAbi::C, link_name, args)?;
                let items = this.read_target_usize(items)?;
                let elem_size = this.read_target_usize(elem_size)?;
                if let Some(size) = this.compute_size_in_bytes(Size::from_bytes(elem_size), items) {
                    let res = this.malloc(size.bytes(), AllocInit::Zero)?;
                    this.write_pointer(res, dest)?;
                } else {
                    // On size overflow, return null and, on Unix, set errno.
                    if this.target_os_is_unix() {
                        this.set_last_error(LibcError("ENOMEM"))?;
                    }
                    this.write_null(dest)?;
                }
            }
            "free" => {
                let [ptr] = this.check_shim_sig_lenient(abi, CanonAbi::C, link_name, args)?;
                let ptr = this.read_pointer(ptr)?;
                this.free(ptr)?;
            }
            "realloc" => {
                let [old_ptr, new_size] =
                    this.check_shim_sig_lenient(abi, CanonAbi::C, link_name, args)?;
                let old_ptr = this.read_pointer(old_ptr)?;
                let new_size = this.read_target_usize(new_size)?;
                if new_size <= this.max_size_of_val().bytes() {
                    let res = this.realloc(old_ptr, new_size)?;
                    this.write_pointer(res, dest)?;
                } else {
                    // If this does not fit in an isize, return null and, on Unix, set errno.
                    if this.target_os_is_unix() {
                        this.set_last_error(LibcError("ENOMEM"))?;
                    }
                    this.write_null(dest)?;
                }
            }

            // C memory handling functions
            "memcmp" => {
                let [left, right, n] =
                    this.check_shim_sig_lenient(abi, CanonAbi::C, link_name, args)?;
                let left = this.read_pointer(left)?;
                let right = this.read_pointer(right)?;
                let n = Size::from_bytes(this.read_target_usize(n)?);

                // C requires that this must always be a valid pointer (C18 §7.1.4).
                this.ptr_get_alloc_id(left, 0)?;
                this.ptr_get_alloc_id(right, 0)?;

                // memcmp does *not* have any wording like `memchr` that says anything about
                // stopping as soon as a difference is found. So we requires both buffers
                // to be fully inbounds and initialized.

                let result = {
                    let left_bytes = this.read_bytes_ptr_strip_provenance(left, n)?;
                    let right_bytes = this.read_bytes_ptr_strip_provenance(right, n)?;

                    use std::cmp::Ordering::*;
                    match left_bytes.cmp(right_bytes) {
                        Less => -1i32,
                        Equal => 0,
                        Greater => 1,
                    }
                };

                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "memchr" => {
                let [ptr, val, num] =
                    this.check_shim_sig_lenient(abi, CanonAbi::C, link_name, args)?;
                let ptr = this.read_pointer(ptr)?;
                let val = this.read_scalar(val)?.to_i32()?;
                let num = this.read_target_usize(num)?;
                // The docs say val is "interpreted as unsigned char".
                #[expect(clippy::as_conversions)]
                let val = val as u8;

                // C requires that this must always be a valid pointer (C18 §7.1.4).
                this.ptr_get_alloc_id(ptr, 0)?;

                // "The implementation shall behave as if it reads the characters sequentially and
                // stops as soon as a matching character is found."
                let needle_ptr = this.memchr(ptr, 0..num, val)?.map(|(_idx, ptr)| ptr);

                if let Some(needle_ptr) = needle_ptr {
                    this.write_pointer(needle_ptr, dest)?;
                } else {
                    this.write_null(dest)?;
                }
            }
            "memrchr" => {
                this.check_target_os(&[Os::Linux, Os::Android, Os::FreeBsd], link_name)?;

                let [ptr, val, num] =
                    this.check_shim_sig_lenient(abi, CanonAbi::C, link_name, args)?;
                let ptr = this.read_pointer(ptr)?;
                let val = this.read_scalar(val)?.to_i32()?;
                let num = this.read_target_usize(num)?;
                // The docs say val is "interpreted as unsigned char".
                #[expect(clippy::as_conversions)]
                let val = val as u8;

                // C requires that this must always be a valid pointer (C18 §7.1.4).
                this.ptr_get_alloc_id(ptr, 0)?;

                // We use the same early-abort search strategy as `memchr` (see above).
                let needle_ptr = this.memchr(ptr, (0..num).rev(), val)?.map(|(_idx, ptr)| ptr);

                if let Some(needle_ptr) = needle_ptr {
                    this.write_pointer(needle_ptr, dest)?;
                } else {
                    this.write_null(dest)?;
                }
            }
            "strlen" => {
                let [ptr] = this.check_shim_sig_lenient(abi, CanonAbi::C, link_name, args)?;
                let ptr = this.read_pointer(ptr)?;
                // This reads at least 1 byte, so we are already enforcing that this is a valid pointer.
                let n = this.read_c_str(ptr)?.len();
                this.write_scalar(
                    Scalar::from_target_usize(u64::try_from(n).unwrap(), this),
                    dest,
                )?;
            }
            "strnlen" => {
                let [ptr, num] = this.check_shim_sig_lenient(abi, CanonAbi::C, link_name, args)?;
                let ptr = this.read_pointer(ptr)?;
                let num = this.read_target_usize(num)?;

                // C requires that this must always be a valid pointer (C18 §7.1.4).
                this.ptr_get_alloc_id(ptr, 0)?;

                // The docs say this behaves like memchr, which only deref's the memory it actually
                // needs to compare.
                let idx = this.memchr(ptr, 0..num, 0)?.map(|(idx, _ptr)| idx).unwrap_or(num);
                this.write_scalar(Scalar::from_target_usize(idx, this), dest)?;
            }
            "wcslen" => {
                let [ptr] = this.check_shim_sig_lenient(abi, CanonAbi::C, link_name, args)?;
                let ptr = this.read_pointer(ptr)?;
                // This reads at least 1 byte, so we are already enforcing that this is a valid pointer.
                let n = this.read_wchar_t_str(ptr)?.len();
                this.write_scalar(
                    Scalar::from_target_usize(u64::try_from(n).unwrap(), this),
                    dest,
                )?;
            }
            "memcpy" => {
                let [ptr_dest, ptr_src, n] =
                    this.check_shim_sig_lenient(abi, CanonAbi::C, link_name, args)?;
                let ptr_dest = this.read_pointer(ptr_dest)?;
                let ptr_src = this.read_pointer(ptr_src)?;
                let n = this.read_target_usize(n)?;

                // C requires that this must always be a valid pointer, even if `n` is zero, so we better check that.
                // (This is more than Rust requires, so `mem_copy` is not sufficient.)
                this.ptr_get_alloc_id(ptr_dest, 0)?;
                this.ptr_get_alloc_id(ptr_src, 0)?;

                this.mem_copy(ptr_src, ptr_dest, Size::from_bytes(n), true)?;
                this.write_pointer(ptr_dest, dest)?;
            }
            "strcpy" => {
                let [ptr_dest, ptr_src] =
                    this.check_shim_sig_lenient(abi, CanonAbi::C, link_name, args)?;
                let ptr_dest = this.read_pointer(ptr_dest)?;
                let ptr_src = this.read_pointer(ptr_src)?;

                // We use `read_c_str` to determine the amount of data to copy,
                // and then use `mem_copy` for the actual copy. This means
                // pointer provenance is preserved by this implementation of `strcpy`.
                // That is probably overly cautious, but there also is no fundamental
                // reason to have `strcpy` destroy pointer provenance.
                // This reads at least 1 byte, so we are already enforcing that this is a valid pointer.
                let n = this.read_c_str(ptr_src)?.len().strict_add(1);
                this.mem_copy(ptr_src, ptr_dest, Size::from_bytes(n), true)?;
                this.write_pointer(ptr_dest, dest)?;
            }
            "memset" => {
                let [ptr_dest, val, n] =
                    this.check_shim_sig_lenient(abi, CanonAbi::C, link_name, args)?;
                let ptr_dest = this.read_pointer(ptr_dest)?;
                let val = this.read_scalar(val)?.to_i32()?;
                let n = this.read_target_usize(n)?;
                // The docs say val is "interpreted as unsigned char".
                #[expect(clippy::as_conversions)]
                let val = val as u8;

                // C requires that this must always be a valid pointer, even if `n` is zero, so we better check that.
                this.ptr_get_alloc_id(ptr_dest, 0)?;

                let bytes = std::iter::repeat_n(val, n.try_into().unwrap());
                this.write_bytes_ptr(ptr_dest, bytes)?;
                this.write_pointer(ptr_dest, dest)?;
            }

            // kmiri
            // Used for tests
            "kern_miri_record_time" => {
                use std::time::{SystemTime, UNIX_EPOCH};
                let [test_id] =
                    this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let _test_id = this.read_target_usize(test_id)?;
                let now = SystemTime::now();
                let since_epoch = now.duration_since(UNIX_EPOCH).unwrap();
                this.machine.record.push(since_epoch);
            }
            // OS thread operations
            "kern_miri_get_cpu_local_base" => {
                let current_cpu = this.machine.threads.active_cpu;
                this.write_scalar(
                    Scalar::from_target_usize(
                        this.machine.threads.cpu_local_base[current_cpu] as u64,
                        this,
                    ),
                    dest,
                )?;
            }
            "kern_miri_set_cpu_local_base" => {
                let [cpu_base] =
                    this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let cpu_base = this.read_target_usize(cpu_base)?;
                let current_cpu = this.machine.threads.active_cpu;
                this.machine.threads.cpu_local_base[current_cpu] = cpu_base as usize;
            }
            "kern_miri_irq_is_enabled" => {
                let [] = this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                if !this.machine.irq_config.enabled() {
                    throw_machine_stop!(TerminationInfo::Abort(
                        "IRQ shim used while KMiri IRQ simulation is disabled".to_owned()
                    ));
                }
                let cpu = this.machine.threads.active_cpu;
                this.write_scalar(Scalar::from_bool(this.machine.irq_cpus[cpu].enabled), dest)?;
            }
            "kern_miri_irq_disable" => {
                let [] = this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                if !this.machine.irq_config.enabled() {
                    throw_machine_stop!(TerminationInfo::Abort(
                        "IRQ shim used while KMiri IRQ simulation is disabled".to_owned()
                    ));
                }
                let cpu = this.machine.threads.active_cpu;
                this.machine.irq_cpus[cpu].enabled = false;
                if this.machine.irq_config.trace() {
                    eprintln!(
                        "KMiri IRQ: cpu={cpu} thread={:?} action=disable block_count={} pending={}",
                        this.machine.threads.active_thread(),
                        this.machine.basic_block_count,
                        this.machine.irq_cpus[cpu].pending.len(),
                    );
                }
            }
            "kern_miri_irq_enable" => {
                let [] = this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                if !this.machine.irq_config.enabled() {
                    throw_machine_stop!(TerminationInfo::Abort(
                        "IRQ shim used while KMiri IRQ simulation is disabled".to_owned()
                    ));
                }
                let cpu = this.machine.threads.active_cpu;
                this.machine.irq_cpus[cpu].enabled = true;
                if this.machine.irq_config.trace() {
                    eprintln!(
                        "KMiri IRQ: cpu={cpu} thread={:?} action=enable block_count={} pending={}",
                        this.machine.threads.active_thread(),
                        this.machine.basic_block_count,
                        this.machine.irq_cpus[cpu].pending.len(),
                    );
                }
            }
            "kern_miri_register_irq_entry" => {
                let [entry] =
                    this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                if !this.machine.irq_config.enabled() {
                    throw_machine_stop!(TerminationInfo::Abort(format!(
                        "{link_name}: IRQ entry registered while KMiri IRQ simulation is disabled"
                    )));
                }
                if this.machine.irq_entry.is_some() {
                    throw_machine_stop!(TerminationInfo::Abort(format!(
                        "{link_name}: IRQ entry already registered"
                    )));
                }
                let entry = this.get_ptr_fn(this.read_pointer(entry)?)?.as_instance()?;
                this.machine.irq_entry = Some(entry);
                if this.machine.irq_config.trace() {
                    eprintln!(
                        "KMiri IRQ: cpu={} action=entry-registered block_count={}",
                        this.machine.threads.active_cpu, this.machine.basic_block_count,
                    );
                }
            }
            "kern_miri_set_timer_vector" | "kern_miri_test_raise_irq" => {
                let [vector] =
                    this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let vector = this.read_scalar(vector)?.to_u8()?;
                if !this.machine.irq_config.enabled() {
                    throw_machine_stop!(TerminationInfo::Abort(format!(
                        "{link_name}: vector {vector} used while KMiri IRQ simulation is disabled"
                    )));
                }
                if vector < 32 {
                    throw_machine_stop!(TerminationInfo::Abort(format!(
                        "{link_name}: vector {vector} is reserved for CPU exceptions (valid range 32..=255)"
                    )));
                }
                let cpu = this.machine.threads.active_cpu;
                if link_name.as_str() == "kern_miri_set_timer_vector" {
                    this.machine.timer_vector = Some(vector);
                    if this.machine.irq_config.trace() {
                        eprintln!(
                            "KMiri IRQ: cpu={cpu} action=timer-vector vector={vector} block_count={}",
                            this.machine.basic_block_count,
                        );
                    }
                } else {
                    if this.machine.irq_entry.is_none() {
                        throw_machine_stop!(TerminationInfo::Abort(format!(
                            "{link_name}: vector {vector} raised before IRQ entry was registered"
                        )));
                    }
                    let event_id =
                        this.machine.queue_irq(cpu, vector, crate::machine::IrqSource::Test);
                    if this.machine.irq_config.trace() {
                        eprintln!(
                            "KMiri IRQ: event={event_id} cpu={cpu} thread={:?} action=queued vector={vector} source=Test block_count={} pending={} IF={}",
                            this.machine.threads.active_thread(),
                            this.machine.basic_block_count,
                            this.machine.irq_cpus[cpu].pending.len(),
                            this.machine.irq_cpus[cpu].enabled,
                        );
                    }
                }
            }
            "kern_miri_init_ap" => {
                let [cpu_id, func, args, task, stack_end, stack_size] =
                    this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let cpu_id = this.read_target_usize(cpu_id)?;
                let start_routine = this.read_pointer(func)?;
                let func_arg = this.read_immediate(args)?;
                let task = this.deref_pointer(task)?;
                let stack_end = this.read_target_usize(stack_end)?;
                let stack_size = this.read_target_usize(stack_size)?;
                let id = this.start_regular_thread(
                    None,
                    start_routine,
                    ExternAbi::Rust,
                    func_arg,
                    this.machine.layouts.unit,
                    Some(Stack::Range {
                        start: (stack_end - stack_size) as usize,
                        end: stack_end as usize,
                    }),
                )?;
                this.set_ap_init_thread(cpu_id as usize, id);
                this.machine.thread_map.try_insert(task.ptr().addr(), id).unwrap();
            }
            "miri_create_new_thread" => {
                let [func, args, task, stack_end, stack_size] =
                    this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let start_routine = this.read_pointer(func)?;
                let func_arg = this.read_immediate(args)?;
                let task = this.deref_pointer(task)?;
                let stack_end = this.read_target_usize(stack_end)?;
                let stack_size = this.read_target_usize(stack_size)?;
                let id = this.start_regular_thread(
                    None,
                    start_routine,
                    ExternAbi::Rust,
                    func_arg,
                    this.machine.layouts.unit,
                    Some(Stack::Range {
                        start: (stack_end - stack_size) as usize,
                        end: stack_end as usize,
                    }),
                )?;
                this.machine.thread_map.insert(task.ptr().addr(), id);
            }
            "miri_switch_to" | "miri_switch_to_after_return" => {
                let [task] = this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let task = this.deref_pointer(task)?;
                let Some(thread_id) = this.machine.thread_map.get(&task.ptr().addr()) else {
                    throw_machine_stop!(TerminationInfo::Abort(
                        "the program aborted execution due to wrong switch".to_owned()
                    ))
                };
                let thread_id = *thread_id;
                if link_name.as_str() == "miri_switch_to" {
                    this.machine.threads.switch_to(thread_id);
                } else {
                    // An exiting kernel task must finish its entry and TLS destructors
                    // before switching away, so its stack is reclaimed normally.
                    let mut on_stack_empty =
                        this.active_thread_mut().on_stack_empty.take().unwrap();
                    this.active_thread_mut().on_stack_empty = Some(Box::new(move |this| {
                        let result = on_stack_empty(this)?;
                        if result.is_ready() {
                            this.machine.threads.switch_to(thread_id);
                        }
                        interp_ok(result)
                    }));
                }
            }
            // OS memory
            "kern_miri_alloc_pages" => {
                let [paddr, count] =
                    this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let paddr = this.read_target_usize(paddr)? as usize;
                let count = this.read_target_usize(count)? as usize;
                let page_size = mirch::page_size();
                for i in 0..count {
                    let addr = paddr + i * page_size;
                    mirch::check_page_state(addr, PageState::Unused)?;
                    mirch::set_page_state(addr, PageState::Untyped);
                    insert_init_mask(this, addr, this.machine.get_default_alloc_params());
                }
            }
            "kern_miri_dealloc_pages" => {
                let [paddr, count] =
                    this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let paddr = this.read_target_usize(paddr)? as usize;
                let count = this.read_target_usize(count)? as usize;
                dealloc_pages(this, paddr, count)?;
            }
            "kern_miri_dealloc_heap_slot" => {
                let [ptr, size] =
                    this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let ptr = this.read_pointer(ptr)?;
                let size = this.read_target_usize(size)? as usize;
                this.dealloc_heap_slot(ptr, size)?;
            }
            "kern_miri_alloc_heap_slot" => {
                let [ptr, size] =
                    this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let ptr = this.read_pointer(ptr)?;
                let size = this.read_target_usize(size)? as usize;
                this.alloc_heap_slot(ptr, size)?;
            }
            "kern_miri_set_heap_slot_next" => {
                let [ptr, next] =
                    this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let ptr = this.read_pointer(ptr)?;
                let next = this.read_target_usize(next)? as usize;
                let addr = ptr.addr().bytes() as usize;
                mirch::physical_mem_mut().heap_slot_next.insert(addr, next);
            }
            "kern_miri_get_heap_slot_next" => {
                let [ptr] =
                    this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let ptr = this.read_pointer(ptr)?;
                let addr = ptr.addr().bytes() as usize;
                let next = mirch::physical_mem_mut().heap_slot_next.remove(&addr).unwrap();
                this.write_scalar(Scalar::from_target_usize(next as u64, this), dest)?;
            }
            "kern_miri_zero" => {
                let [paddr, page_count] =
                    this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let paddr = this.read_target_usize(paddr)? as usize;
                let page_count = this.read_target_usize(page_count)? as usize;
                let page_size = mirch::page_size();
                // Only pages allocated by `kern_miri_alloc_pages` have an init mask. Check all of
                // them before writing so that a bad range does not touch the physical memory.
                for i in 0..page_count {
                    let paddr_page = paddr + i * page_size;
                    if !mirch::physical_mem().init_masks.contains_key(&paddr_page) {
                        throw_ub_format!(
                            "Page state UB: zeroing page (paddr=0x{paddr_page:x}) that was not allocated by kern_miri_alloc_pages"
                        );
                    }
                }
                let actual_ptr = mirch::paddr_to_mem(paddr);
                unsafe {
                    core::ptr::write_bytes(actual_ptr, 0, page_count * page_size);
                }
                for i in 0..page_count {
                    let paddr_page = paddr + i * page_size;
                    let init_masks = &mut mirch::physical_mem_mut().init_masks;
                    let mask_allocation = init_masks.get_mut(&paddr_page).unwrap();
                    // Notify the allocation that it has been initialized.
                    let _ = mask_allocation
                        .get_bytes_unchecked_for_overwrite_ptr(this, (0..page_size).into());
                }
            }
            "kern_miri_write_bytes" => {
                let [paddr, data, len] =
                    this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let paddr = this.read_target_usize(paddr)? as usize;
                let bytes = {
                    let data = this.read_pointer(data)?;
                    let len = this.read_target_usize(len)?;
                    this.read_bytes_ptr_strip_provenance(data, Size::from_bytes(len))?
                };
                mirch::physical_write_bytes(paddr, bytes, this)?;
            }
            "kern_miri_retype_pages" => {
                let [paddr, count, page_type, slot_size] =
                    this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let paddr = this.read_target_usize(paddr)? as usize;
                let count = this.read_target_usize(count)? as usize;
                let page_type = this.read_target_usize(page_type)? as usize;
                let slot_size = this.read_target_usize(slot_size)? as usize;
                let page_size = mirch::page_size();
                assert_eq!(page_size % slot_size, 0);
                for page_index in 0..count {
                    mirch::check_page_state(paddr + page_index * page_size, PageState::Untyped)?;
                }
                type_pages_at(paddr, count, slot_size, TypedKind::from_usize(page_type).unwrap())?;
            }
            "kern_miri_get_root_page_table" => {
                let root_paddr = mirch::physical_mem().page_table.as_ref().unwrap().root_paddr();
                this.write_scalar(Scalar::from_target_usize(root_paddr as u64, this), dest)?;
            }
            "kern_miri_set_root_page_table" => {
                let [paddr] = this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let paddr = this.read_target_usize(paddr)? as usize;
                mirch::set_page_table(PageTable::new(paddr));
            }
            "kern_miri_get_cpu_local_va" => {
                let [cpu_local_va] =
                    this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let vaddr = this.read_target_usize(cpu_local_va)? as usize;
                this.write_scalar(Scalar::from_target_usize(vaddr as u64, this), dest)?;
                // let cpu = this.machine.threads.active_cpu;
                // if cpu == 0 {
                //     this.write_scalar(
                //         Scalar::from_target_usize(vaddr as u64, this),
                //         dest,
                //     )?;
                // } else {
                //     let actual_vaddr = *this.machine.cpu_alloc.borrow().get(&(cpu, vaddr)).unwrap_or_else(|| {
                //         println!("something wrong: {:x}", vaddr);
                //         println!("{:?}", this.machine.cpu_alloc);
                //         panic!();
                //     });
                //     this.write_scalar(
                //         Scalar::from_target_usize(actual_vaddr as u64, this),
                //         dest,
                //     )?;
                //     // else {
                //     //     let alloc_id = this.alloc_id_from_addr()
                //     //     let mut global_state = this.machine.alloc_addresses.borrow_mut();
                //     //     let old_allocation = {
                //     //         let alloc_id = global_state.base_ad
                //     //     };
                //     //     let allocation = Allocation::uninit()
                //     // }
                // }
            }
            "kern_miri_copy_untyped" => {
                let [dst, src, len] =
                    this.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let dst = this.read_target_usize(dst)? as usize;
                let src = this.read_target_usize(src)? as usize;
                let len = this.read_target_usize(len)? as usize;
                mirch::physical_copy(dst, src, len)?;
            }

            // Rust for Linux symbols
            "rust_helper_krealloc_node_align" | "krealloc_node_align" => {
                let [old_ptr, new_size, align, flags, _nid] =
                    this.check_shim_sig_lenient(abi, CanonAbi::C, link_name, args)?;
                let old_ptr = this.read_pointer(old_ptr)?;
                let new_size = this.read_target_usize(new_size)?;
                if new_size == 0 {
                    if !this.ptr_is_null(old_ptr)? {
                        this.free(old_ptr)?;
                    }
                    // RFL replaces this return value with its own aligned dangling pointer.
                    this.write_null(dest)?;
                } else {
                    let align = this.read_target_usize(align)?;
                    let Ok(align) = Align::from_bytes(align) else {
                        throw_unsup_format!("`{link_name}`: unsupported alignment {align}");
                    };
                    // RFL's Kmalloc pads the size to its requested alignment, as required
                    // by kmalloc's alignment guarantees. Retain fundamental alignment too.
                    let align = align.max(this.malloc_align(new_size));
                    let flags = this.read_scalar(flags)?.to_u32()?;
                    // Linux include/linux/gfp_types.h: ___GFP_ZERO_BIT is 8.
                    const GFP_ZERO: u32 = 1 << 8;
                    let init =
                        if flags & GFP_ZERO != 0 { AllocInit::Zero } else { AllocInit::Uninit };
                    // Model logical allocations, not NUMA placement or GFP scheduling.
                    // Linux's full zeroing guarantee requires __GFP_ZERO on every call
                    // from the initial allocation onward. Reallocation preserves the old
                    // prefix (including its init mask), and `init` initializes new bytes.
                    let res = if this.ptr_is_null(old_ptr)? {
                        this.allocate_ptr(
                            Size::from_bytes(new_size),
                            align,
                            MiriMemoryKind::C.into(),
                            init,
                        )?
                    } else {
                        this.reallocate_ptr(
                            old_ptr,
                            None,
                            Size::from_bytes(new_size),
                            align,
                            MiriMemoryKind::C.into(),
                            init,
                        )?
                    };
                    this.write_pointer(res, dest)?;
                }
            }

            // Fallback to shims in submodules.
            _ => {
                if let res = shims::kmiri_passthrough::EvalContextExt::emulate_bitmap_passthrough(
                    this, link_name, abi, args, dest,
                )? && !matches!(res, EmulateItemResult::NotSupported)
                {
                    return interp_ok(res);
                }

                // Math shims
                if let res = shims::math::EvalContextExt::emulate_foreign_item_inner(
                    this, link_name, abi, args, dest,
                )? && !matches!(res, EmulateItemResult::NotSupported)
                {
                    return interp_ok(res);
                }

                // Platform-specific shims
                return match &this.tcx.sess.target.os {
                    _ if this.target_os_is_unix() =>
                        shims::unix::foreign_items::EvalContextExt::emulate_foreign_item_inner(
                            this, link_name, abi, args, dest,
                        ),
                    Os::Windows =>
                        shims::windows::foreign_items::EvalContextExt::emulate_foreign_item_inner(
                            this, link_name, abi, args, dest,
                        ),
                    _ => interp_ok(EmulateItemResult::NotSupported),
                };
            }
        };
        // We only fall through to here if we did *not* hit the `_` arm above,
        // i.e., if we actually emulated the function with one of the shims.
        interp_ok(EmulateItemResult::NeedsReturn)
    }

    /// For each `idx` yielded by `idxs`, check if `ptr + idx` equals `needle` and return that
    /// index and a pointer to that element if so. Return `None` if none of the indices match.
    fn memchr(
        &self,
        ptr: Pointer,
        idxs: impl Iterator<Item = u64>,
        needle: u8,
    ) -> InterpResult<'tcx, Option<(u64, Pointer)>> {
        let this = self.eval_context_ref();
        for idx in idxs {
            let ptr = ptr.wrapping_offset(Size::from_bytes(idx), this);
            let place = this.ptr_to_mplace(ptr, this.machine.layouts.u8);
            let val = this.read_scalar(&place)?.to_u8()?;
            if val == needle {
                return interp_ok(Some((idx, ptr)));
            }
        }
        interp_ok(None)
    }
}
