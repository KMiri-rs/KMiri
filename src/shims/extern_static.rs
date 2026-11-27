//! Provides the `extern static` that this platform expects.

use rustc_abi::{Align, Size};
use rustc_middle::mir::interpret::{self, AllocInit};
use rustc_span::Symbol;
use rustc_target::spec::{Env, Os};

use crate::*;

impl<'tcx> MiriMachine<'tcx> {
    fn add_extern_static(ecx: &mut MiriInterpCx<'tcx>, name: &str, ptr: Pointer) {
        // This got just allocated, so there definitely is a pointer here.
        let ptr = ptr.into_pointer_or_addr().unwrap();
        ecx.machine.extern_statics.try_insert(Symbol::intern(name), ptr).unwrap();
    }

    fn alloc_extern_static(
        ecx: &mut MiriInterpCx<'tcx>,
        name: &str,
        val: ImmTy<'tcx>,
    ) -> InterpResult<'tcx, Pointer> {
        let place = ecx.allocate(val.layout, MiriMemoryKind::ExternStatic.into())?;
        ecx.write_immediate(*val, &place)?;
        Self::add_extern_static(ecx, name, place.ptr());
        interp_ok(place.ptr())
    }

    /// Make `ptr` available as a weak symbol with the given name.
    fn add_weak_symbol(
        ecx: &mut MiriInterpCx<'tcx>,
        name: &str,
        ptr: Pointer,
    ) -> InterpResult<'tcx> {
        // Allocate the extra indirection place and add it to the map.
        let layout = ecx.machine.layouts.unit_ptr_mut;
        let place = ecx.allocate(layout, MiriMemoryKind::ExternStatic.into())?;
        ecx.write_scalar(Scalar::from_maybe_pointer(ptr, ecx), &place)?;
        let weak_ptr = place.ptr().into_pointer_or_addr().unwrap();
        ecx.machine.extern_statics_imports.try_insert(Symbol::intern(name), weak_ptr).unwrap();
        interp_ok(())
    }

    /// Extern statics that are initialized with function pointers to the symbols of the same name.
    fn weak_fn_symbols(ecx: &mut MiriInterpCx<'tcx>, names: &[&str]) -> InterpResult<'tcx> {
        for name in names {
            assert!(ecx.is_dyn_sym(name), "{name} is not a dynamic symbol");
            let ptr = ecx.fn_ptr(FnVal::Other(DynSym::from_str(name)));
            Self::add_weak_symbol(ecx, name, ptr.into())?;
        }
        interp_ok(())
    }

    /// Plays the linker for the configured link sections: the statics placed in
    /// a section are copied, in exported-symbol order (which need not be the
    /// link order), into one read-only array, and its start
    /// and end symbols point at the first element and one past the last.
    /// Each static is its own allocation, so walking from one item to the next
    /// needs this array; writes to the original statics are not reflected in it.
    fn init_link_section_arrays(ecx: &mut MiriInterpCx<'tcx>) -> InterpResult<'tcx> {
        let Some(toml) = ecx.machine.kmiri_toml.as_ref() else { return interp_ok(()) };
        let sections = toml.link_sections().to_vec();
        let tcx = ecx.tcx.tcx;
        for section in sections {
            let items = helpers::link_section_statics(tcx, &section.name)?;
            let mut elem: Option<(Size, Align)> = None;
            for &def_id in &items {
                let ty = tcx.type_of(def_id).instantiate_identity().skip_norm_wip();
                let layout = ecx.layout_of(ty)?;
                match elem {
                    None => elem = Some((layout.size, layout.align)),
                    Some((size, _)) if size != layout.size =>
                        throw_unsup_format!(
                            "link section `{}` holds statics of different sizes",
                            section.name
                        ),
                    Some((size, align)) => elem = Some((size, align.max(layout.align))),
                }
            }
            // An empty section still needs an allocation for its symbols; it is
            // pointer-sized so that a declaration of a pointer type fits.
            let (size, align) =
                elem.unwrap_or((ecx.pointer_size(), ecx.tcx.data_layout.pointer_align()));
            let count = u64::try_from(items.len()).unwrap();
            let total = if items.is_empty() { size } else { size * count };
            let base = ecx.allocate_ptr(
                total,
                align,
                MiriMemoryKind::ExternStatic.into(),
                AllocInit::Zero,
            )?;
            for (i, &def_id) in items.iter().enumerate() {
                let alloc_id = tcx.reserve_and_set_static_alloc(def_id);
                let src = ecx.global_root_pointer(interpret::Pointer::from(alloc_id))?;
                let dst = base.wrapping_offset(size * u64::try_from(i).unwrap(), ecx);
                ecx.mem_copy(src.into(), dst.into(), size, true)?;
            }
            ecx.alloc_mark_immutable(base.provenance.get_alloc_id().unwrap())?;
            let end = base.wrapping_offset(size * count, ecx);
            Self::add_extern_static(ecx, &section.start, base.into());
            Self::add_extern_static(ecx, &section.end, end.into());
        }
        interp_ok(())
    }

    /// Sets up the "extern statics" for this machine.
    pub fn init_extern_statics(ecx: &mut MiriInterpCx<'tcx>) -> InterpResult<'tcx> {
        if ecx.target_os_is_unix() {
            // "environ" is mandated by POSIX.
            let environ = ecx.machine.env_vars.unix().environ();
            Self::add_extern_static(ecx, "environ", environ);
            // We also provide it as a weak symbol, which is needed on FreeBSD.
            Self::add_weak_symbol(ecx, "environ", environ)?;
        }

        match &ecx.tcx.sess.target.os {
            Os::Linux => {
                Self::weak_fn_symbols(
                    ecx,
                    &["copy_file_range", "getrandom", "gettid", "statx", "strlen"],
                )?;
                if ecx.is_dyn_sym("__futimens64") {
                    Self::weak_fn_symbols(ecx, &["__futimens64"])?;
                }
                if ecx.tcx.sess.target.env == Env::Gnu {
                    // FIXME: enabling this with genmc causes test failures because of how
                    // std then uses pthread keys and genmc does not know about their state.
                    if ecx.machine.data_race.as_genmc_ref().is_none() {
                        Self::weak_fn_symbols(ecx, &["__cxa_thread_atexit_impl"])?;
                    }
                    // The __dso_handle symbol just marks a unique global address.
                    let val = ImmTy::from_int(0, ecx.machine.layouts.u8);
                    let ptr = Self::alloc_extern_static(ecx, "__dso_handle", val)?;
                    // We also need a weak symbol for `__dso_handle`.
                    Self::add_weak_symbol(ecx, "__dso_handle", ptr)?;
                }
            }
            Os::Android => {
                Self::weak_fn_symbols(
                    ecx,
                    &["signal", "getrandom", "gettid", "futimens", "preadv", "pwritev"],
                )?;
            }
            Os::Windows => {
                // "_tls_used"
                // This is some obscure hack that is part of the Windows TLS story. It's a `u8`.
                let val = ImmTy::from_int(0, ecx.machine.layouts.u8);
                Self::alloc_extern_static(ecx, "_tls_used", val)?;
            }
            Os::Illumos | Os::Solaris => {
                Self::weak_fn_symbols(ecx, &["pthread_setname_np"])?;
            }
            _ => {} // No "extern statics" supported on this target.
        }

        Self::init_link_section_arrays(ecx)?;

        // Also initialize `missing_weak_symbol`.
        let place = ecx.allocate(ecx.machine.layouts.usize, MiriMemoryKind::ExternStatic.into())?;
        ecx.write_int(0, &place)?;
        ecx.machine.extern_static_weak_import_default =
            Some(place.ptr().into_pointer_or_addr().unwrap());

        interp_ok(())
    }
}
