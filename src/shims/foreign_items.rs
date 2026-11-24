#![allow(clippy::arithmetic_side_effects)]
use std::collections::hash_map::Entry;
use std::io::Write;
use std::mem;
use std::path::Path;

use rustc_abi::{Align, CanonAbi, ExternAbi, Size};
use rustc_ast::expand::allocator::NO_ALLOC_SHIM_IS_UNSTABLE;
use rustc_attr_ir::Linkage;
use rustc_data_structures::either::Either;
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
use crate::eval::{ExitType, ProgramExitHandler};
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
        let ecx = self.eval_context_mut();

        // Handle allocator shim.
        if let Some(shim) = ecx.machine.allocator_shim_symbols.get(&link_name) {
            match *shim {
                Either::Left(other_fn) => {
                    let handler = ecx
                        .lookup_exported_fn(other_fn)?
                        .expect("missing alloc error handler symbol");
                    return interp_ok(Some(handler));
                }
                Either::Right(special) => {
                    ecx.rust_special_allocator_method(special, link_name, abi, args, dest)?;
                    ecx.return_to_block(ret)?;
                    return interp_ok(None);
                }
            }
        }

        // FIXME: avoid allocating memory
        let dest = ecx.force_allocation(dest)?;

        // If the user configured this symbol to prefer their own implementation,
        // skip the built-in shim entirely and delegate to the user's body directly.
        if ecx
            .machine
            .kmiri_toml
            .as_ref()
            .is_some_and(|c| c.prefers_user_implementation(link_name.as_str()))
        {
            if let Some(body) = ecx.lookup_exported_fn(link_name)? {
                return interp_ok(Some(body));
            }
        }

        // The rest either implements the logic, or falls back to `lookup_exported_symbol`.
        let res = ecx.emulate_foreign_item_inner(link_name, abi, args, &dest)?;
        res.jump_to_next_block(ecx, &dest.clone().into(), ret, Some(unwind), |ecx| {
            if let Some(body) = ecx.lookup_exported_fn(link_name)? {
                return interp_ok(Some(body));
            }

            throw_machine_stop!(TerminationInfo::UnsupportedForeignItem(format!(
                "can't call foreign function `{link_name}` on OS `{os}`",
                os = ecx.tcx.sess.target.os,
            )));
        })
    }

    fn is_dyn_sym(&self, name: &str) -> bool {
        let ecx = self.eval_context_ref();
        match &ecx.tcx.sess.target.os {
            _ if ecx.target_os_is_unix() =>
                shims::unix::foreign_items::is_dyn_sym(name, &ecx.tcx.sess.target),
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

    /// Lookup the instance that has `link_name` as the symbol name.
    fn lookup_exported_symbol(
        &self,
        link_name: Symbol,
    ) -> InterpResult<'tcx, Option<ty::Instance<'tcx>>> {
        let ecx = self.eval_context_ref();
        let tcx = ecx.tcx.tcx;

        // If the result was cached, just return it.
        // (Cannot use `or_insert` since the code below might have to throw an error.)
        let mut cache = ecx.machine.exported_symbols_cache.borrow_mut();
        let entry = cache.entry(link_name);
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
                helpers::iter_exported_symbols(tcx, |cnum, def_id, _used| {
                    let attrs = tcx.codegen_fn_attrs(def_id);
                    // Skip over items without an explicitly defined symbol name.
                    if !(attrs.symbol_name.is_some()
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
                                    let original_span = tcx.def_span(original.instance.def_id());
                                    let span = tcx.def_span(def_id);
                                    if original_span.lo_hi() < span.lo_hi() {
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

                e.insert(symbol_target.map(|SymbolTarget { instance, .. }| instance))
            }
        };
        drop(cache);
        interp_ok(instance)
    }

    /// Lookup the body of a function that has `link_name` as the symbol name.
    fn lookup_exported_fn(
        &self,
        link_name: Symbol,
    ) -> InterpResult<'tcx, Option<(&'tcx mir::Body<'tcx>, ty::Instance<'tcx>)>> {
        let ecx = self.eval_context_ref();
        let instance = ecx.lookup_exported_symbol(link_name)?;
        if let Some(instance) = &instance {
            if !matches!(ecx.tcx.def_kind(instance.def_id()), DefKind::Fn | DefKind::AssocFn) {
                throw_ub_format!(
                    "attempt to call an exported symbol that is not defined as a function"
                );
            }
        }
        match instance {
            None => interp_ok(None),
            Some(instance) => interp_ok(Some((ecx.load_mir(instance.def, None)?, instance))),
        }
    }

    /// Lookup the instance of a static that has `link_name` as the symbol name.
    fn lookup_exported_static(
        &self,
        link_name: Symbol,
    ) -> InterpResult<'tcx, Option<ty::Instance<'tcx>>> {
        let ecx = self.eval_context_ref();
        let instance = ecx.lookup_exported_symbol(link_name)?;
        if let Some(instance) = &instance {
            if !matches!(ecx.tcx.def_kind(instance.def_id()), DefKind::Static { .. }) {
                throw_ub_format!(
                    "attempt to access an exported symbol `{link_name}` that is not defined as a static"
                );
            }
        }
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
        let ecx = self.eval_context_mut();

        // First deal with any external C functions in linked .so file.
        #[cfg(all(feature = "native-lib", unix))]
        if !ecx.machine.native_lib.is_empty() {
            use crate::shims::native_lib::EvalContextExt as _;
            // An Ok(false) here means that the function being called was not exported
            // by the specified `.so` file; we should continue and check if it corresponds to
            // a provided shim.
            if ecx.call_native_fn(link_name, dest, args)? {
                return interp_ok(EmulateItemResult::NeedsReturn);
            }
        }
        // When adding a new shim, you should follow the following pattern:
        // ```
        // "shim_name" => {
        //     let [arg1, arg2, arg3] = &this.check_shim_sig(
        //         shim_sig!(extern "C" fn(usize, *libc::some_type, *i32) -> u32),
        //         (link_name, abi, args),
        //     )?;
        //     this.shim_name(arg1, arg2, arg3, dest)?;
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
        //     dest: &MPlaceTy<'tcx>)
        // -> InterpResult<'tcx, Scalar> {
        //     let ecx = self.eval_context_mut();
        //
        //     // First thing: load all the arguments. Details depend on the shim.
        //     let arg1 = ecx.read_scalar(arg1)?.to_u32()?;
        //     let arg2 = ecx.read_pointer(arg2)?; // when you need to work with the pointer directly
        //     let arg3 = ecx.deref_pointer(arg3)?; // when you want to load/store through the pointer
        //
        //     // ...
        //
        //     ecx.write_int(42, dest)?;
        //     interp_ok(())
        // }
        // ```
        // You might find existing shims not following this pattern, most
        // likely because they predate it or because for some reason they cannot be made to fit.

        // Here we dispatch all the shims for foreign functions. If you have a platform specific
        // shim, add it to the corresponding submodule.
        match link_name.as_str() {
            // Magic function Rust emits (and not as part of the allocator shim).
            name if name == ecx.mangle_internal_symbol(NO_ALLOC_SHIM_IS_UNSTABLE) => {
                // This is a no-op shim that only exists to prevent making the allocator shims
                // instantly stable.
                let [] = &ecx
                    .check_shim_sig(shim_sig!(extern "Rust" fn() -> ()), (link_name, abi, args))?;
            }

            // Miri-specific extern functions
            "miri_alloc" => {
                let [size, align] = &ecx.check_shim_sig(
                    shim_sig!(extern "Rust" fn(usize, usize) -> *()),
                    (link_name, abi, args),
                )?;
                let size = ecx.read_target_usize(size)?;
                let align = ecx.read_target_usize(align)?;

                ecx.check_rust_alloc_request(size, align)?;

                let ptr = ecx.allocate_ptr(
                    Size::from_bytes(size),
                    Align::from_bytes(align).unwrap(),
                    MiriMemoryKind::Miri.into(),
                    AllocInit::Uninit,
                )?;

                ecx.write_pointer(ptr, dest)?;
            }
            "miri_dealloc" => {
                let [ptr, old_size, align] = &ecx.check_shim_sig(
                    shim_sig!(extern "Rust" fn(*(), usize, usize) -> ()),
                    (link_name, abi, args),
                )?;
                let ptr = ecx.read_pointer(ptr)?;
                let old_size = ecx.read_target_usize(old_size)?;
                let align = ecx.read_target_usize(align)?;

                // No need to check old_size/align; we anyway check that they match the allocation.
                ecx.deallocate_ptr(
                    ptr,
                    Some((Size::from_bytes(old_size), Align::from_bytes(align).unwrap())),
                    MiriMemoryKind::Miri.into(),
                )?;
            }
            "miri_track_alloc" => {
                let [ptr] = &ecx.check_shim_sig(
                    shim_sig!(extern "Rust" fn(*()) -> ()),
                    (link_name, abi, args),
                )?;
                let ptr = ecx.read_pointer(ptr)?;
                let (alloc_id, _, _) = ecx.ptr_get_alloc_id(ptr, 0).map_err_kind(|_e| {
                    err_machine_stop!(TerminationInfo::Abort(format!(
                        "pointer passed to `miri_get_alloc_id` must not be dangling, got {ptr:?}"
                    )))
                })?;
                if ecx.machine.tracked_alloc_ids.insert(alloc_id) {
                    let info = ecx.get_alloc_info(alloc_id);
                    ecx.emit_diagnostic(NonHaltingDiagnostic::TrackingAlloc(
                        alloc_id, info.size, info.align,
                    ));
                }
            }
            "miri_start_unwind" => {
                let [payload] = &ecx.check_shim_sig(
                    shim_sig!(extern "Rust" fn(*()) -> !),
                    (link_name, abi, args),
                )?;
                ecx.handle_miri_start_unwind(payload)?;
                return interp_ok(EmulateItemResult::NeedsUnwind);
            }
            "miri_run_provenance_gc" => {
                let [] = &ecx
                    .check_shim_sig(shim_sig!(extern "Rust" fn() -> ()), (link_name, abi, args))?;
                ecx.run_provenance_gc();
            }
            "miri_get_alloc_id" => {
                let [ptr] = &ecx.check_shim_sig(
                    shim_sig!(extern "Rust" fn(*()) -> u64),
                    (link_name, abi, args),
                )?;
                let ptr = ecx.read_pointer(ptr)?;
                let (alloc_id, _, _) = ecx.ptr_get_alloc_id(ptr, 0).map_err_kind(|_e| {
                    err_machine_stop!(TerminationInfo::Abort(format!(
                        "pointer passed to `miri_get_alloc_id` must not be dangling, got {ptr:?}"
                    )))
                })?;
                ecx.write_scalar(Scalar::from_u64(alloc_id.0.get()), dest)?;
            }
            "miri_print_borrow_state" => {
                let [id, show_unnamed] = &ecx.check_shim_sig(
                    shim_sig!(extern "Rust" fn(u64, bool) -> ()),
                    (link_name, abi, args),
                )?;
                let id = ecx.read_scalar(id)?.to_u64()?;
                let show_unnamed = ecx.read_scalar(show_unnamed)?.to_bool()?;
                if let Some(id) = std::num::NonZero::new(id).map(AllocId)
                    && ecx.get_alloc_info(id).kind == AllocKind::LiveData
                {
                    ecx.print_borrow_state(id, show_unnamed)?;
                } else {
                    eprintln!("{id} is not the ID of a live data allocation");
                }
            }
            "miri_pointer_name" => {
                // This associates a name to a tag. Very useful for debugging, and also makes
                // tests more strict.
                let [ptr, nth_parent, name] = &ecx.check_shim_sig(
                    shim_sig!(extern "Rust" fn(*(), u8, &[u8]) -> ()),
                    (link_name, abi, args),
                )?;
                let ptr = ecx.read_pointer(ptr)?;
                let nth_parent = ecx.read_scalar(nth_parent)?.to_u8()?;
                let name = ecx.read_immediate(name)?;

                let name = ecx.read_byte_slice(&name)?;
                // We must make `name` owned because we need to
                // end the shared borrow from `read_byte_slice` before we can
                // start the mutable borrow for `give_pointer_debug_name`.
                let name = String::from_utf8_lossy(name).into_owned();
                ecx.give_pointer_debug_name(ptr, nth_parent, &name)?;
            }
            "miri_static_root" => {
                let [ptr] = &ecx.check_shim_sig(
                    shim_sig!(extern "Rust" fn(*()) -> ()),
                    (link_name, abi, args),
                )?;
                let ptr = ecx.read_pointer(ptr)?;
                let (alloc_id, offset, _) = ecx.ptr_get_alloc_id(ptr, 0)?;
                if offset != Size::ZERO {
                    throw_unsup_format!(
                        "pointer passed to `miri_static_root` must point to beginning of an allocated block"
                    );
                }
                ecx.machine.static_roots.push(alloc_id);
            }
            "miri_host_to_target_path" => {
                let [ptr, out, out_size] = &ecx.check_shim_sig(
                    shim_sig!(extern "Rust" fn(*(), *(), usize) -> usize),
                    (link_name, abi, args),
                )?;
                let ptr = ecx.read_pointer(ptr)?;
                let out = ecx.read_pointer(out)?;
                let out_size = ecx.read_scalar(out_size)?.to_target_usize(ecx)?;

                // The host affects program behavior here, so this requires isolation to be disabled.
                ecx.check_no_isolation("`miri_host_to_target_path`")?;

                // We read this as a plain OsStr and write it as a path, which will convert it to the target.
                let path = ecx.read_os_str_from_c_str(ptr)?.to_owned();
                let (success, needed_size) =
                    ecx.write_path_to_c_str(Path::new(&path), out, out_size)?;
                // Return value: 0 on success, otherwise the size it would have needed.
                ecx.write_int(if success { 0 } else { needed_size }, dest)?;
            }
            "miri_thread_spawn" => {
                let [start_routine, func_arg] = &ecx.check_shim_sig(
                    // FIXME: The first argument is actually a function pointer.
                    shim_sig!(extern "Rust" fn(fn(..) -> _, *()) -> usize),
                    (link_name, abi, args),
                )?;
                let start_routine = ecx.read_pointer(start_routine)?;
                let func_arg = ecx.read_immediate(func_arg)?;

                ecx.start_regular_thread(
                    Some(dest.clone()),
                    start_routine,
                    ExternAbi::Rust,
                    func_arg,
                    ecx.machine.layouts.unit,
                    None,
                )?;
            }
            "miri_thread_join" => {
                let [thread_id] = &ecx.check_shim_sig(
                    shim_sig!(extern "Rust" fn(usize) -> bool),
                    (link_name, abi, args),
                )?;

                let thread = ecx.read_target_usize(thread_id)?;
                // Joining a terminated thread is valid.
                use crate::concurrency::thread::ThreadLookupError;
                let thread = match ecx.thread_id_try_from(thread) {
                    Ok(id) | Err(ThreadLookupError::Terminated(id)) => Some(id),
                    Err(ThreadLookupError::InvalidId) => None,
                };
                if let Some(thread) = thread {
                    ecx.join_thread_exclusive(
                        thread,
                        /* success_retval */ Scalar::from_bool(true),
                        dest,
                    )?;
                } else {
                    ecx.write_scalar(Scalar::from_bool(false), dest)?;
                }
            }
            // Hint that a loop is spinning indefinitely.
            "miri_spin_loop" => {
                let [] = &ecx
                    .check_shim_sig(shim_sig!(extern "Rust" fn() -> ()), (link_name, abi, args))?;

                // Try to run another thread to maximize the chance of finding actual bugs.
                ecx.yield_active_thread();
            }
            // Obtains the size of a Miri backtrace. See the README for details.
            "miri_backtrace_size" => {
                ecx.handle_miri_backtrace_size(abi, link_name, args, dest)?;
            }
            // Obtains a Miri backtrace. See the README for details.
            "miri_get_backtrace" => {
                // `check_shim` happens inside `handle_miri_get_backtrace`.
                ecx.handle_miri_get_backtrace(abi, link_name, args)?;
            }
            // Resolves a Miri backtrace frame. See the README for details.
            "miri_resolve_frame" => {
                // `check_shim` happens inside `handle_miri_resolve_frame`.
                ecx.handle_miri_resolve_frame(abi, link_name, args, dest)?;
            }
            // Writes the function and file names of a Miri backtrace frame into a user provided buffer. See the README for details.
            "miri_resolve_frame_names" => {
                ecx.handle_miri_resolve_frame_names(abi, link_name, args)?;
            }
            // Writes some bytes to the interpreter's stdout/stderr. See the README for details.
            "miri_write_to_stdout" | "miri_write_to_stderr" => {
                let [msg] = &ecx.check_shim_sig(
                    shim_sig!(extern "Rust" fn(&[u8]) -> ()),
                    (link_name, abi, args),
                )?;
                let msg = ecx.read_immediate(msg)?;
                let msg = ecx.read_byte_slice(&msg)?;
                if ecx.machine.debugger.is_some() {
                    ecx.machine
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

                let [ptr, align] = &ecx.check_shim_sig(
                    shim_sig!(extern "Rust" fn(*(), usize) -> ()),
                    (link_name, abi, args),
                )?;

                let ptr = ecx.read_pointer(ptr)?;
                let align = ecx.read_target_usize(align)?;
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
                if let Ok((alloc_id, offset, ..)) = ecx.ptr_try_get_alloc_id(ptr, 0) {
                    let alloc_align = ecx.get_alloc_info(alloc_id).align;
                    // If the newly promised alignment is bigger than the native alignment of this
                    // allocation, and bigger than the previously promised alignment, then set it.
                    if align > alloc_align
                        && ecx
                            .machine
                            .symbolic_alignment
                            .get_mut()
                            .get(&alloc_id)
                            .is_none_or(|&(_, old_align)| align > old_align)
                    {
                        ecx.machine.symbolic_alignment.get_mut().insert(alloc_id, (offset, align));
                    }
                }
            }
            // GenMC mode: Assume statements block the current thread when their condition is false.
            "miri_genmc_assume" => {
                let [condition] = &ecx.check_shim_sig(
                    shim_sig!(extern "Rust" fn(bool) -> ()),
                    (link_name, abi, args),
                )?;

                if ecx.machine.data_race.as_genmc_ref().is_some() {
                    ecx.handle_genmc_verifier_assume(condition)?;
                } else {
                    throw_unsup_format!("miri_genmc_assume is only supported in GenMC mode")
                }
            }

            // Aborting the process.
            "exit" => {
                let [exit_code] = &ecx
                    .check_shim_sig(shim_sig!(extern "C" fn(i32) -> ()), (link_name, abi, args))?;
                let exit_code = ecx.read_scalar(exit_code)?.to_i32()?;
                // Ensure `exit` gets called at most once.
                let old_exit = mem::replace(&mut ecx.machine.exit_called, true);
                if old_exit {
                    throw_ub_format!("`exit` called more than once");
                }
                // Initialize the at-exit state machine.
                ecx.replace_active_thread_stack({
                    let mut state_machine =
                        ProgramExitHandler::new(exit_code, ExitType::ExitCalled);
                    Box::new(move |m| state_machine.on_stack_empty(m))
                });
                // We "jumped" to the exit handling.
                return interp_ok(EmulateItemResult::AlreadyJumped);
            }
            "abort" => {
                // FIXME: This does not have a direct test (#3179).
                let [] =
                    &ecx.check_shim_sig(shim_sig!(extern "C" fn() -> ()), (link_name, abi, args))?;
                throw_machine_stop!(TerminationInfo::Abort(
                    "the program aborted execution".to_owned()
                ));
            }

            // Standard C allocation
            "malloc" => {
                let [size] = &ecx.check_shim_sig(
                    shim_sig!(extern "C" fn(usize) -> *_),
                    (link_name, abi, args),
                )?;
                let size = ecx.read_target_usize(size)?;
                if size <= ecx.max_size_of_val().bytes() {
                    let res = ecx.malloc(size, AllocInit::Uninit)?;
                    ecx.write_pointer(res, dest)?;
                } else {
                    // If this does not fit in an isize, return null and, on Unix, set errno.
                    if ecx.target_os_is_unix() {
                        ecx.set_last_error(LibcError("ENOMEM"))?;
                    }
                    ecx.write_int(0, dest)?;
                }
            }
            "calloc" => {
                let [items, elem_size] = &ecx.check_shim_sig(
                    shim_sig!(extern "C" fn(usize, usize) -> *_),
                    (link_name, abi, args),
                )?;
                let items = ecx.read_target_usize(items)?;
                let elem_size = ecx.read_target_usize(elem_size)?;
                if let Some(size) = ecx.compute_size_in_bytes(Size::from_bytes(elem_size), items) {
                    let res = ecx.malloc(size.bytes(), AllocInit::Zero)?;
                    ecx.write_pointer(res, dest)?;
                } else {
                    // On size overflow, return null and, on Unix, set errno.
                    if ecx.target_os_is_unix() {
                        ecx.set_last_error(LibcError("ENOMEM"))?;
                    }
                    ecx.write_int(0, dest)?;
                }
            }
            "free" => {
                let [ptr] = &ecx
                    .check_shim_sig(shim_sig!(extern "C" fn(*_) -> ()), (link_name, abi, args))?;
                let ptr = ecx.read_pointer(ptr)?;
                ecx.free(ptr)?;
            }
            "realloc" => {
                let [old_ptr, new_size] = &ecx.check_shim_sig(
                    shim_sig!(extern "C" fn(*_, usize) -> *_),
                    (link_name, abi, args),
                )?;
                let old_ptr = ecx.read_pointer(old_ptr)?;
                let new_size = ecx.read_target_usize(new_size)?;
                if new_size <= ecx.max_size_of_val().bytes() {
                    let res = ecx.realloc(old_ptr, new_size)?;
                    ecx.write_pointer(res, dest)?;
                } else {
                    // If this does not fit in an isize, return null and, on Unix, set errno.
                    if ecx.target_os_is_unix() {
                        ecx.set_last_error(LibcError("ENOMEM"))?;
                    }
                    ecx.write_int(0, dest)?;
                }
            }
            "malloc_usable_size" => {
                ecx.check_target_os(&[Os::Linux, Os::FreeBsd, Os::Android], link_name)?;

                let [ptr] = &ecx.check_shim_sig(
                    shim_sig!(extern "C" fn(*_) -> usize),
                    (link_name, abi, args),
                )?;
                let ptr = ecx.read_pointer(ptr)?;
                let size = if ecx.ptr_is_null(ptr)? {
                    0
                } else {
                    let (alloc_id, offset, _) = ecx.ptr_get_alloc_id(ptr, 0)?;
                    if offset.bytes() != 0 {
                        throw_ub_format!(
                            "`malloc_usable_size` was called on a pointer that does not point to the beginning of its allocation"
                        );
                    }
                    let Some((alloc_kind, _)) = ecx.memory.alloc_map().get(alloc_id) else {
                        throw_ub_format!(
                            "`malloc_usable_size` was called on a pointer to memory not managed by the C allocator"
                        );
                    };
                    if *alloc_kind != MiriMemoryKind::C.into() {
                        throw_ub_format!(
                            "`malloc_usable_size` was called on a pointer to {alloc_kind} memory, which is not managed by the C allocator"
                        );
                    }
                    ecx.get_alloc_info(alloc_id).size.bytes()
                };
                ecx.write_scalar(Scalar::from_target_usize(size, ecx), dest)?;
            }

            // C memory handling functions
            "memcmp" => {
                let [left, right, n] = &ecx.check_shim_sig(
                    shim_sig!(extern "C" fn(*_, *_, usize) -> i32),
                    (link_name, abi, args),
                )?;
                let left = ecx.read_pointer(left)?;
                let right = ecx.read_pointer(right)?;
                let n = Size::from_bytes(ecx.read_target_usize(n)?);

                // C requires that this must always be a valid pointer (C18 §7.1.4).
                ecx.ptr_get_alloc_id(left, 0)?;
                ecx.ptr_get_alloc_id(right, 0)?;

                // memcmp does *not* have any wording like `memchr` that says anything about
                // stopping as soon as a difference is found. So we requires both buffers
                // to be fully inbounds and initialized.

                let result = {
                    let left_bytes = ecx.read_bytes_ptr_strip_provenance(left, n)?;
                    let right_bytes = ecx.read_bytes_ptr_strip_provenance(right, n)?;

                    use std::cmp::Ordering::*;
                    match left_bytes.cmp(right_bytes) {
                        Less => -1i32,
                        Equal => 0,
                        Greater => 1,
                    }
                };

                ecx.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "memchr" => {
                let [ptr, val, num] = &ecx.check_shim_sig(
                    shim_sig!(extern "C" fn(*_, i32, usize) -> *_),
                    (link_name, abi, args),
                )?;
                let ptr = ecx.read_pointer(ptr)?;
                let val = ecx.read_scalar(val)?.to_i32()?;
                let num = ecx.read_target_usize(num)?;
                // The docs say val is "interpreted as unsigned char".
                #[expect(clippy::as_conversions)]
                let val = val as u8;

                // C requires that this must always be a valid pointer (C18 §7.1.4).
                ecx.ptr_get_alloc_id(ptr, 0)?;

                // "The implementation shall behave as if it reads the characters sequentially and
                // stops as soon as a matching character is found."
                let needle_ptr = ecx.memchr(ptr, 0..num, val)?.map(|(_idx, ptr)| ptr);

                if let Some(needle_ptr) = needle_ptr {
                    ecx.write_pointer(needle_ptr, dest)?;
                } else {
                    ecx.write_int(0, dest)?;
                }
            }
            "memrchr" => {
                ecx.check_target_os(&[Os::Linux, Os::Android, Os::FreeBsd], link_name)?;

                let [ptr, val, num] = &ecx.check_shim_sig(
                    shim_sig!(extern "C" fn(*_, i32, usize) -> *_),
                    (link_name, abi, args),
                )?;
                let ptr = ecx.read_pointer(ptr)?;
                let val = ecx.read_scalar(val)?.to_i32()?;
                let num = ecx.read_target_usize(num)?;
                // The docs say val is "interpreted as unsigned char".
                #[expect(clippy::as_conversions)]
                let val = val as u8;

                // C requires that this must always be a valid pointer (C18 §7.1.4).
                ecx.ptr_get_alloc_id(ptr, 0)?;

                // We use the same early-abort search strategy as `memchr` (see above).
                let needle_ptr = ecx.memchr(ptr, (0..num).rev(), val)?.map(|(_idx, ptr)| ptr);

                if let Some(needle_ptr) = needle_ptr {
                    ecx.write_pointer(needle_ptr, dest)?;
                } else {
                    ecx.write_int(0, dest)?;
                }
            }
            "strlen" => {
                let [ptr] = &ecx.check_shim_sig(
                    shim_sig!(extern "C" fn(*_) -> usize),
                    (link_name, abi, args),
                )?;
                let ptr = ecx.read_pointer(ptr)?;
                // This reads at least 1 byte, so we are already enforcing that this is a valid pointer.
                let n = ecx.read_c_str(ptr)?.len();
                ecx.write_scalar(Scalar::from_target_usize(u64::try_from(n).unwrap(), ecx), dest)?;
            }
            "strnlen" => {
                let [ptr, num] = &ecx.check_shim_sig(
                    shim_sig!(extern "C" fn(*_, usize) -> usize),
                    (link_name, abi, args),
                )?;
                let ptr = ecx.read_pointer(ptr)?;
                let num = ecx.read_target_usize(num)?;

                // C requires that this must always be a valid pointer (C18 §7.1.4).
                ecx.ptr_get_alloc_id(ptr, 0)?;

                // The docs say this behaves like memchr, which only deref's the memory it actually
                // needs to compare.
                let idx = ecx.memchr(ptr, 0..num, 0)?.map(|(idx, _ptr)| idx).unwrap_or(num);
                ecx.write_scalar(Scalar::from_target_usize(idx, ecx), dest)?;
            }
            "wcslen" => {
                let [ptr] = &ecx.check_shim_sig(
                    shim_sig!(extern "C" fn(*_) -> usize),
                    (link_name, abi, args),
                )?;
                let ptr = ecx.read_pointer(ptr)?;
                // This reads at least 1 byte, so we are already enforcing that this is a valid pointer.
                let n = ecx.read_wchar_t_str(ptr)?.len();
                ecx.write_scalar(Scalar::from_target_usize(u64::try_from(n).unwrap(), ecx), dest)?;
            }
            "memcpy" => {
                let [ptr_dest, ptr_src, n] = &ecx.check_shim_sig(
                    shim_sig!(extern "C" fn(*_, *_, usize) -> *_),
                    (link_name, abi, args),
                )?;
                let ptr_dest = ecx.read_pointer(ptr_dest)?;
                let ptr_src = ecx.read_pointer(ptr_src)?;
                let n = ecx.read_target_usize(n)?;

                // C requires that this must always be a valid pointer, even if `n` is zero, so we better check that.
                // (This is more than Rust requires, so `mem_copy` is not sufficient.)
                ecx.ptr_get_alloc_id(ptr_dest, 0)?;
                ecx.ptr_get_alloc_id(ptr_src, 0)?;

                ecx.mem_copy(ptr_src, ptr_dest, Size::from_bytes(n), true)?;
                ecx.write_pointer(ptr_dest, dest)?;
            }
            "strcpy" => {
                let [ptr_dest, ptr_src] = &ecx.check_shim_sig(
                    shim_sig!(extern "C" fn(*_, *_) -> *_),
                    (link_name, abi, args),
                )?;
                let ptr_dest = ecx.read_pointer(ptr_dest)?;
                let ptr_src = ecx.read_pointer(ptr_src)?;

                // We use `read_c_str` to determine the amount of data to copy,
                // and then use `mem_copy` for the actual copy. This means
                // pointer provenance is preserved by this implementation of `strcpy`.
                // That is probably overly cautious, but there also is no fundamental
                // reason to have `strcpy` destroy pointer provenance.
                // This reads at least 1 byte, so we are already enforcing that this is a valid pointer.
                let n = ecx.read_c_str(ptr_src)?.len().strict_add(1);
                ecx.mem_copy(ptr_src, ptr_dest, Size::from_bytes(n), true)?;
                ecx.write_pointer(ptr_dest, dest)?;
            }
            "memset" => {
                let [ptr_dest, val, n] = &ecx.check_shim_sig(
                    shim_sig!(extern "C" fn(*_, i32, usize) -> *_),
                    (link_name, abi, args),
                )?;
                let ptr_dest = ecx.read_pointer(ptr_dest)?;
                let val = ecx.read_scalar(val)?.to_i32()?;
                let n = ecx.read_target_usize(n)?;
                // The docs say val is "interpreted as unsigned char".
                #[expect(clippy::as_conversions)]
                let val = val as u8;

                // C requires that this must always be a valid pointer, even if `n` is zero, so we better check that.
                ecx.ptr_get_alloc_id(ptr_dest, 0)?;

                let bytes = std::iter::repeat_n(val, n.try_into().unwrap());
                ecx.write_bytes_ptr(ptr_dest, bytes)?;
                ecx.write_pointer(ptr_dest, dest)?;
            }

            // kmiri
            // Used for tests
            // Returns a pointer to the `index`-th static placed in the link section whose
            // name is the UTF-8 string at `name` (`name_len` bytes), or null past the end.
            // A kernel's linker script collects such statics into one array between two
            // symbols. In KMiri every static is its own allocation, so offsetting from one
            // item to the next would leave the allocation; the kernel walks the section one
            // item at a time instead. Items are listed in exported-symbol order.
            "kern_miri_link_section_item" => {
                let [name, name_len, index] =
                    ecx.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let name = ecx.read_pointer(name)?;
                let name_len = ecx.read_target_usize(name_len)?;
                let index = ecx.read_target_usize(index)?;
                let name = ecx.read_bytes_ptr_strip_provenance(name, Size::from_bytes(name_len))?;
                let Ok(name) = String::from_utf8(name.to_vec()) else {
                    throw_unsup_format!("`kern_miri_link_section_item`: section name is not UTF-8");
                };
                let tcx = ecx.tcx.tcx;
                let mut items = vec![];
                helpers::iter_exported_symbols(tcx, |_cnum, def_id, _used| {
                    if !tcx.is_foreign_item(def_id)
                        && matches!(tcx.def_kind(def_id), DefKind::Static { .. })
                        && !tcx.is_thread_local_static(def_id)
                        && tcx
                            .codegen_fn_attrs(def_id)
                            .link_section
                            .is_some_and(|s| s.as_str() == name)
                    {
                        items.push(def_id);
                    }
                    interp_ok(())
                })?;
                match usize::try_from(index).ok().and_then(|i| items.get(i)) {
                    Some(&def_id) => {
                        let alloc_id = tcx.reserve_and_set_static_alloc(def_id);
                        let ptr = ecx.global_root_pointer(
                            rustc_middle::mir::interpret::Pointer::from(alloc_id),
                        )?;
                        ecx.write_pointer(ptr, dest)?;
                    }
                    None => ecx.write_int(0, dest)?,
                }
            }
            "kern_miri_record_time" => {
                use std::time::{SystemTime, UNIX_EPOCH};
                let [test_id] = ecx.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let _test_id = ecx.read_target_usize(test_id)?;
                let now = SystemTime::now();
                let since_epoch = now.duration_since(UNIX_EPOCH).unwrap();
                ecx.machine.record.push(since_epoch);
            }
            // OS thread operations
            "kern_miri_get_cpu_local_base" => {
                let current_cpu = ecx.machine.threads.active_cpu;
                ecx.write_scalar(
                    Scalar::from_target_usize(
                        ecx.machine.threads.cpu_local_base[current_cpu] as u64,
                        ecx,
                    ),
                    dest,
                )?;
            }
            "kern_miri_set_cpu_local_base" => {
                let [cpu_base] =
                    ecx.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let cpu_base = ecx.read_target_usize(cpu_base)?;
                let current_cpu = ecx.machine.threads.active_cpu;
                ecx.machine.threads.cpu_local_base[current_cpu] = cpu_base as usize;
            }
            "kern_miri_irq_is_enabled" => {
                let [] = ecx.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                if !ecx.machine.irq_config.enabled() {
                    throw_machine_stop!(TerminationInfo::Abort(
                        "IRQ shim used while KMiri IRQ simulation is disabled".to_owned()
                    ));
                }
                let cpu = ecx.machine.threads.active_cpu;
                ecx.write_scalar(Scalar::from_bool(ecx.machine.irq_cpus[cpu].enabled), dest)?;
            }
            "kern_miri_irq_disable" => {
                let [] = ecx.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                if !ecx.machine.irq_config.enabled() {
                    throw_machine_stop!(TerminationInfo::Abort(
                        "IRQ shim used while KMiri IRQ simulation is disabled".to_owned()
                    ));
                }
                let cpu = ecx.machine.threads.active_cpu;
                ecx.machine.irq_cpus[cpu].enabled = false;
                if ecx.machine.irq_config.trace() {
                    eprintln!(
                        "KMiri IRQ: cpu={cpu} thread={:?} action=disable block_count={} pending={}",
                        ecx.machine.threads.active_thread(),
                        ecx.machine.basic_block_count,
                        ecx.machine.irq_cpus[cpu].pending.len(),
                    );
                }
            }
            "kern_miri_irq_enable" => {
                let [] = ecx.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                if !ecx.machine.irq_config.enabled() {
                    throw_machine_stop!(TerminationInfo::Abort(
                        "IRQ shim used while KMiri IRQ simulation is disabled".to_owned()
                    ));
                }
                let cpu = ecx.machine.threads.active_cpu;
                ecx.machine.irq_cpus[cpu].enabled = true;
                if ecx.machine.irq_config.trace() {
                    eprintln!(
                        "KMiri IRQ: cpu={cpu} thread={:?} action=enable block_count={} pending={}",
                        ecx.machine.threads.active_thread(),
                        ecx.machine.basic_block_count,
                        ecx.machine.irq_cpus[cpu].pending.len(),
                    );
                }
            }
            "kern_miri_register_irq_entry" => {
                let [entry] = ecx.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                if !ecx.machine.irq_config.enabled() {
                    throw_machine_stop!(TerminationInfo::Abort(format!(
                        "{link_name}: IRQ entry registered while KMiri IRQ simulation is disabled"
                    )));
                }
                if ecx.machine.irq_entry.is_some() {
                    throw_machine_stop!(TerminationInfo::Abort(format!(
                        "{link_name}: IRQ entry already registered"
                    )));
                }
                let entry = ecx.get_ptr_fn(ecx.read_pointer(entry)?)?.as_instance()?;
                ecx.machine.irq_entry = Some(entry);
                if ecx.machine.irq_config.trace() {
                    eprintln!(
                        "KMiri IRQ: cpu={} action=entry-registered block_count={}",
                        ecx.machine.threads.active_cpu, ecx.machine.basic_block_count,
                    );
                }
            }
            "kern_miri_set_timer_vector" | "kern_miri_test_raise_irq" => {
                let [vector] = ecx.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let vector = ecx.read_scalar(vector)?.to_u8()?;
                if !ecx.machine.irq_config.enabled() {
                    throw_machine_stop!(TerminationInfo::Abort(format!(
                        "{link_name}: vector {vector} used while KMiri IRQ simulation is disabled"
                    )));
                }
                if vector < 32 {
                    throw_machine_stop!(TerminationInfo::Abort(format!(
                        "{link_name}: vector {vector} is reserved for CPU exceptions (valid range 32..=255)"
                    )));
                }
                let cpu = ecx.machine.threads.active_cpu;
                if link_name.as_str() == "kern_miri_set_timer_vector" {
                    ecx.machine.timer_vector = Some(vector);
                    if ecx.machine.irq_config.trace() {
                        eprintln!(
                            "KMiri IRQ: cpu={cpu} action=timer-vector vector={vector} block_count={}",
                            ecx.machine.basic_block_count,
                        );
                    }
                } else {
                    if ecx.machine.irq_entry.is_none() {
                        throw_machine_stop!(TerminationInfo::Abort(format!(
                            "{link_name}: vector {vector} raised before IRQ entry was registered"
                        )));
                    }
                    let event_id =
                        ecx.machine.queue_irq(cpu, vector, crate::machine::IrqSource::Test);
                    if ecx.machine.irq_config.trace() {
                        eprintln!(
                            "KMiri IRQ: event={event_id} cpu={cpu} thread={:?} action=queued vector={vector} source=Test block_count={} pending={} IF={}",
                            ecx.machine.threads.active_thread(),
                            ecx.machine.basic_block_count,
                            ecx.machine.irq_cpus[cpu].pending.len(),
                            ecx.machine.irq_cpus[cpu].enabled,
                        );
                    }
                }
            }
            // The size of the machine's RAM, which starts at physical address 0: what a
            // firmware memory map reports to the kernel.
            "kern_miri_ram_size" => {
                let [] = ecx.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let size = mirch::total_mem_size();
                ecx.write_scalar(Scalar::from_target_usize(size.try_into().unwrap(), ecx), dest)?;
            }
            "kern_miri_cpu_count" => {
                let [] = ecx.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let num_cpus = ecx.machine.threads.num_cpus();
                ecx.write_scalar(
                    Scalar::from_target_usize(num_cpus.try_into().unwrap(), ecx),
                    dest,
                )?;
            }
            "kern_miri_current_cpu" => {
                let [] = ecx.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let cpu = ecx.machine.threads.active_cpu;
                ecx.write_scalar(Scalar::from_target_usize(cpu.try_into().unwrap(), ecx), dest)?;
            }
            "kern_miri_start_cpu" => {
                // Starts CPU `cpu`, which is off, at `func(arg)` on the given stack, like a
                // startup IPI. Everything the caller did before is visible to the new CPU's
                // first step: the same edge as creating a thread.
                let [cpu, func, arg, stack_end, stack_size] =
                    ecx.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let cpu = ecx.read_target_usize(cpu)?;
                let start_routine = ecx.read_pointer(func)?;
                let func_arg = ecx.read_immediate(arg)?;
                let stack_end = ecx.read_target_usize(stack_end)?;
                let stack_size = ecx.read_target_usize(stack_size)?;
                let num_cpus = ecx.machine.threads.num_cpus();
                let Some(cpu) = usize::try_from(cpu).ok().filter(|&cpu| cpu < num_cpus) else {
                    throw_machine_stop!(TerminationInfo::Abort(format!(
                        "the program tried to start CPU {cpu}, but the machine has {num_cpus} CPUs"
                    )))
                };
                if ecx.machine.threads.is_cpu_started(cpu) {
                    throw_machine_stop!(TerminationInfo::Abort(format!(
                        "the program tried to start CPU {cpu}, which is already running"
                    )))
                }
                let id = ecx.start_regular_thread(
                    None,
                    start_routine,
                    ExternAbi::Rust,
                    func_arg,
                    ecx.machine.layouts.unit,
                    Some(Stack::Range {
                        start: (stack_end - stack_size) as usize,
                        end: stack_end as usize,
                    }),
                )?;
                ecx.machine.kernel_threads.insert(id);
                ecx.machine.threads.start_cpu(cpu, id);
                // A CPU comes out of reset with interrupts disabled.
                ecx.machine.irq_cpus[cpu].enabled = false;
            }
            "miri_create_new_thread" => {
                // Returns the new thread's id. OSTD stores it in the task's saved
                // context, where native code keeps the registers to resume from,
                // so the task can still be switched to after its `Task` value moves.
                let [func, args, stack_end, stack_size] =
                    ecx.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let start_routine = ecx.read_pointer(func)?;
                let func_arg = ecx.read_immediate(args)?;
                let stack_end = ecx.read_target_usize(stack_end)?;
                let stack_size = ecx.read_target_usize(stack_size)?;
                let id = ecx.start_regular_thread(
                    None,
                    start_routine,
                    ExternAbi::Rust,
                    func_arg,
                    ecx.machine.layouts.unit,
                    Some(Stack::Range {
                        start: (stack_end - stack_size) as usize,
                        end: stack_end as usize,
                    }),
                )?;
                ecx.machine.kernel_threads.insert(id);
                ecx.write_scalar(Scalar::from_target_usize(id.to_u32().into(), ecx), dest)?;
            }
            "miri_switch_to" | "miri_switch_to_after_return" => {
                // The argument is the thread id read from the next task's saved
                // context. Natively, a context that does not hold a valid resume
                // point crashes the CPU; here it stops the program.
                let [thread] = ecx.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let thread = ecx.read_target_usize(thread)?;
                let thread_id = u32::try_from(thread).ok().map(ThreadId::new_unchecked);
                let Some(thread_id) = thread_id.filter(|id| {
                    ecx.machine.kernel_threads.contains(id)
                        && !ecx.machine.threads.thread_ref(*id).is_terminated()
                }) else {
                    throw_machine_stop!(TerminationInfo::Abort(
                        "the program aborted execution due to wrong switch".to_owned()
                    ))
                };
                if link_name.as_str() == "miri_switch_to" {
                    if let Some(data_race) = ecx.machine.data_race.as_vclocks_ref() {
                        data_race.hand_over_cpu(&ecx.machine.threads, thread_id);
                    }
                    ecx.machine.threads.switch_to(thread_id);
                } else {
                    // An exiting kernel task must finish its entry and TLS destructors
                    // before switching away, so its stack is reclaimed normally.
                    let mut on_stack_empty = ecx.active_thread_mut().on_stack_empty.take().unwrap();
                    ecx.active_thread_mut().on_stack_empty = Some(Box::new(move |this| {
                        let result = on_stack_empty(this)?;
                        if result.is_ready() {
                            if let Some(data_race) = this.machine.data_race.as_vclocks_ref() {
                                data_race.hand_over_cpu(&this.machine.threads, thread_id);
                            }
                            this.machine.threads.switch_to(thread_id);
                        }
                        interp_ok(result)
                    }));
                }
            }
            // OS memory
            "kern_miri_alloc_pages" => {
                let [paddr, count] =
                    ecx.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let paddr = ecx.read_target_usize(paddr)? as usize;
                let count = ecx.read_target_usize(count)? as usize;
                let page_size = mirch::page_size();
                for i in 0..count {
                    let addr = paddr + i * page_size;
                    mirch::check_page_state(addr, PageState::Unused)?;
                    mirch::set_page_state(addr, PageState::Untyped);
                    insert_init_mask(ecx, addr, ecx.machine.get_default_alloc_params());
                }
            }
            "kern_miri_dealloc_pages" => {
                let [paddr, count] =
                    ecx.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let paddr = ecx.read_target_usize(paddr)? as usize;
                let count = ecx.read_target_usize(count)? as usize;
                dealloc_pages(ecx, paddr, count)?;
            }
            "kern_miri_dealloc_heap_slot" => {
                let [ptr, size] =
                    ecx.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let ptr = ecx.read_pointer(ptr)?;
                let size = ecx.read_target_usize(size)? as usize;
                ecx.dealloc_heap_slot(ptr, size)?;
            }
            "kern_miri_alloc_heap_slot" => {
                let [ptr, size] =
                    ecx.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let ptr = ecx.read_pointer(ptr)?;
                let size = ecx.read_target_usize(size)? as usize;
                ecx.alloc_heap_slot(ptr, size)?;
            }
            "kern_miri_zero" => {
                let [paddr, page_count] =
                    ecx.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let paddr = ecx.read_target_usize(paddr)? as usize;
                let page_count = ecx.read_target_usize(page_count)? as usize;
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
                        .get_bytes_unchecked_for_overwrite_ptr(ecx, (0..page_size).into());
                }
            }
            "kern_miri_write_bytes" => {
                let [paddr, data, len] =
                    ecx.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let paddr = ecx.read_target_usize(paddr)? as usize;
                let bytes = {
                    let data = ecx.read_pointer(data)?;
                    let len = ecx.read_target_usize(len)?;
                    ecx.read_bytes_ptr_strip_provenance(data, Size::from_bytes(len))?
                };
                mirch::physical_write_bytes(paddr, bytes, ecx)?;
            }
            "kern_miri_retype_pages" => {
                let [paddr, count, page_type, slot_size] =
                    ecx.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let paddr = ecx.read_target_usize(paddr)? as usize;
                let count = ecx.read_target_usize(count)? as usize;
                let page_type = ecx.read_target_usize(page_type)? as usize;
                let slot_size = ecx.read_target_usize(slot_size)? as usize;
                let page_size = mirch::page_size();
                let page_type = TypedKind::from_usize(page_type).unwrap();
                // `Slab::<SLOT_SIZE>` accepts any slot size up to the page
                // size; one that does not divide the page size leaves a partial
                // slot at the page end (see `lazy_alloc_typed_slot_allocation`).
                if page_type == TypedKind::Slab {
                    assert!(0 < slot_size && slot_size <= page_size);
                } else {
                    assert_eq!(page_size % slot_size, 0);
                }
                for page_index in 0..count {
                    mirch::check_page_state(paddr + page_index * page_size, PageState::Untyped)?;
                }
                type_pages_at(paddr, count, slot_size, page_type)?;
                if let Some(data_race) = ecx.machine.data_race.as_vclocks_ref() {
                    let now = data_race.active_thread_now(&ecx.machine.threads);
                    let mut made_at = ecx.machine.typed_page_made_at.borrow_mut();
                    for page_index in 0..count {
                        made_at.insert(paddr / page_size + page_index, now);
                    }
                }
            }
            "kern_miri_get_root_page_table" => {
                let root_paddr = mirch::physical_mem().page_table.as_ref().unwrap().root_paddr();
                ecx.write_scalar(Scalar::from_target_usize(root_paddr as u64, ecx), dest)?;
            }
            "kern_miri_set_root_page_table" => {
                let [paddr] = ecx.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let paddr = ecx.read_target_usize(paddr)? as usize;
                mirch::set_page_table(PageTable::new(paddr));
            }
            "kern_miri_copy_cpu_local_section" => {
                let [dst] = ecx.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let dst = ecx.read_target_usize(dst)? as usize;
                ecx.copy_cpu_local_section(dst)?;
            }
            "kern_miri_get_cpu_local_va" => {
                let [cpu_local_va] =
                    ecx.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let vaddr = ecx.read_target_usize(cpu_local_va)? as usize;
                ecx.write_scalar(Scalar::from_target_usize(vaddr as u64, ecx), dest)?;
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
            // x86 port I/O. No device is modelled behind any port, so the bus
            // floats, as on a PC where nothing decodes the port: a read returns
            // all ones for the access width and a write is dropped. A kernel's
            // drivers then take their own "device absent" paths.
            "kern_miri_port_read" => {
                let [port, size] =
                    ecx.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let _port = ecx.read_scalar(port)?.to_u16()?;
                let size = ecx.read_target_usize(size)?;
                let value: u32 = match size {
                    1 => 0xff,
                    2 => 0xffff,
                    4 => 0xffff_ffff,
                    _ => throw_unsup_format!("`{link_name}`: unsupported access size {size}"),
                };
                ecx.write_scalar(Scalar::from_u32(value), dest)?;
            }
            "kern_miri_port_write" => {
                let [port, size, value] =
                    ecx.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let _port = ecx.read_scalar(port)?.to_u16()?;
                let size = ecx.read_target_usize(size)?;
                let _value = ecx.read_scalar(value)?.to_u32()?;
                if !matches!(size, 1 | 2 | 4) {
                    throw_unsup_format!("`{link_name}`: unsupported access size {size}");
                }
            }
            "kern_miri_copy_untyped" => {
                let [dst, src, len] =
                    ecx.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let dst = ecx.read_target_usize(dst)? as usize;
                let src = ecx.read_target_usize(src)? as usize;
                let len = ecx.read_target_usize(len)? as usize;
                mirch::physical_copy(dst, src, len)?;
            }
            // Atomic access to a kernel address that may name raw physical memory
            // (the kernel's fallible atomics, e.g. on a futex word in a frame).
            "kern_miri_atomic_load_u32" => {
                let [ptr] = ecx.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let ptr = ecx.read_pointer(ptr)?;
                let value = mirch::kernel_atomic_u32(ecx, ptr, None)?;
                ecx.write_scalar(Scalar::from_u32(value), dest)?;
            }
            "kern_miri_atomic_cmpxchg_u32" => {
                let [ptr, expected, new] =
                    ecx.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                let ptr = ecx.read_pointer(ptr)?;
                let expected = ecx.read_scalar(expected)?.to_u32()?;
                let new = ecx.read_scalar(new)?.to_u32()?;
                let value = mirch::kernel_atomic_u32(ecx, ptr, Some((expected, new)))?;
                ecx.write_scalar(Scalar::from_u32(value), dest)?;
            }
            "kern_miri_copy" => {
                let [dst, src, len] =
                    ecx.check_shim_sig_lenient(abi, CanonAbi::Rust, link_name, args)?;
                // Pointers or plain integer addresses.
                let dst = ecx.read_pointer(dst)?;
                let src = ecx.read_pointer(src)?;
                let len = ecx.read_target_usize(len)? as usize;
                mirch::kernel_copy(ecx, dst, src, len)?;
            }

            // Rust for Linux symbols
            "rust_helper_krealloc_node_align" | "krealloc_node_align" => {
                let [old_ptr, new_size, align, flags, _nid] =
                    ecx.check_shim_sig_lenient(abi, CanonAbi::C, link_name, args)?;
                let old_ptr = ecx.read_pointer(old_ptr)?;
                let new_size = ecx.read_target_usize(new_size)?;
                if new_size == 0 {
                    if !ecx.ptr_is_null(old_ptr)? {
                        ecx.free(old_ptr)?;
                    }
                    // RFL replaces this return value with its own aligned dangling pointer.
                    ecx.write_int(0, dest)?;
                } else {
                    let align = ecx.read_target_usize(align)?;
                    let Ok(align) = Align::from_bytes(align) else {
                        throw_unsup_format!("`{link_name}`: unsupported alignment {align}");
                    };
                    // RFL's Kmalloc pads the size to its requested alignment, as required
                    // by kmalloc's alignment guarantees. Retain fundamental alignment too.
                    let align = align.max(ecx.malloc_align(new_size));
                    let flags = ecx.read_scalar(flags)?.to_u32()?;
                    // Linux include/linux/gfp_types.h: ___GFP_ZERO_BIT is 8.
                    const GFP_ZERO: u32 = 1 << 8;
                    let init =
                        if flags & GFP_ZERO != 0 { AllocInit::Zero } else { AllocInit::Uninit };
                    // Model logical allocations, not NUMA placement or GFP scheduling.
                    // Linux's full zeroing guarantee requires __GFP_ZERO on every call
                    // from the initial allocation onward. Reallocation preserves the old
                    // prefix (including its init mask), and `init` initializes new bytes.
                    let res = if ecx.ptr_is_null(old_ptr)? {
                        ecx.allocate_ptr(
                            Size::from_bytes(new_size),
                            align,
                            MiriMemoryKind::C.into(),
                            init,
                        )?
                    } else {
                        ecx.reallocate_ptr(
                            old_ptr,
                            None,
                            Size::from_bytes(new_size),
                            align,
                            MiriMemoryKind::C.into(),
                            init,
                        )?
                    };
                    ecx.write_pointer(res, dest)?;
                }
            }

            // Fallback to shims in submodules.
            _ => {
                if let res = shims::kmiri_passthrough::EvalContextExt::emulate_bitmap_passthrough(
                    ecx, link_name, abi, args, dest,
                )? && !matches!(res, EmulateItemResult::NotSupported)
                {
                    return interp_ok(res);
                }

                // Math shims
                if let res = shims::math::EvalContextExt::emulate_foreign_item_inner(
                    ecx, link_name, abi, args, dest,
                )? && !matches!(res, EmulateItemResult::NotSupported)
                {
                    return interp_ok(res);
                }

                // Platform-specific shims
                return match &ecx.tcx.sess.target.os {
                    _ if ecx.target_os_is_unix() =>
                        shims::unix::foreign_items::EvalContextExt::emulate_foreign_item_inner(
                            ecx, link_name, abi, args, dest,
                        ),
                    Os::Windows =>
                        shims::windows::foreign_items::EvalContextExt::emulate_foreign_item_inner(
                            ecx, link_name, abi, args, dest,
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
        let ecx = self.eval_context_ref();
        for idx in idxs {
            let ptr = ptr.wrapping_offset(Size::from_bytes(idx), ecx);
            let place = ecx.ptr_to_mplace(ptr, ecx.machine.layouts.u8);
            let val = ecx.read_scalar(&place)?.to_u8()?;
            if val == needle {
                return interp_ok(Some((idx, ptr)));
            }
        }
        interp_ok(None)
    }
}
