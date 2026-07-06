use kmiri_helper::FunctionInstanceInfo;
use rustc_data_structures::fx::FxHashSet;
use rustc_middle::ty::{Instance, TyCtxt};
use rustc_public::{
    CrateDef,
    mir::{MirVisitor, visit::Location},
    rustc_internal::internal,
    ty::{FnDef, GenericArgs, RigidTy, Ty},
};
use rustc_span::{BytePos, FileName, RealFileName, Span, source_map::SourceMap};

fn new_info<'tcx>(instance: Instance<'tcx>, tcx: TyCtxt<'tcx>) -> FunctionInstanceInfo {
    let def_id = instance.def_id();
    let name = tcx.def_path_str(def_id);

    let span = if let Some(local_def_id) = def_id.as_local() {
        let hir_id = tcx.local_def_id_to_hir_id(local_def_id);
        tcx.hir_span_with_body(hir_id)
    } else {
        // Spans from extenal crates are inaccurate, because they point to function header but not the body.
        tcx.def_span(def_id)
    };
    let sm = tcx.sess.source_map();

    FunctionInstanceInfo {
        instance: name,
        source_file: source_file(sm, span),
        line_start: pos_to_line_nr(sm, span.lo()),
        line_end: pos_to_line_nr(sm, span.hi()),
    }
}

pub fn source_file(sm: &SourceMap, span: Span) -> String {
    // Force path remapping, because `prefer_remapped_unconditionally` doesn't always work.
    // Use `--remap-path-prefix` to shorten the long sysroot path, e.g.
    // ./miri run tests/pass/debugger_test.rs --debugger --remap-path-prefix=$(rustc --print=sysroot)/lib/rustlib/src/rust/library/=
    match sm.span_to_filename(span) {
        FileName::Real(path) if let Some(local_path) = path.clone().into_local_path() => {
            FileName::Real(
                sm.path_mapping()
                    .to_real_filename(&RealFileName::empty(), local_path),
            )
        }
        file_name => file_name,
    }
    .prefer_remapped_unconditionally()
    .to_string()
}

pub fn pos_to_line_nr(sm: &SourceMap, pos: BytePos) -> u16 {
    let loc = sm.lookup_char_pos(pos);
    u16::try_from(loc.line).unwrap_or(0)
}

pub struct CollectInstance<'tcx> {
    pub fn_def: FxHashSet<(FnDef, GenericArgs)>,
    pub tcx: TyCtxt<'tcx>,
}

impl MirVisitor for CollectInstance<'_> {
    fn visit_ty(&mut self, ty: &Ty, _: Location) {
        if let Some(RigidTy::FnDef(fn_def, args)) = ty.kind().rigid() {
            self.fn_def.insert((*fn_def, args.clone()));
        }
        self.super_ty(ty);
    }
}

impl<'tcx> CollectInstance<'tcx> {
    pub fn new(tcx: TyCtxt<'tcx>) -> Self {
        CollectInstance {
            fn_def: FxHashSet::default(),
            tcx,
        }
    }

    pub fn into_info(self) -> Vec<FunctionInstanceInfo> {
        let mut v_fn_info = Vec::with_capacity(dbg!(self.fn_def.len()));
        for (fn_def, args) in self.fn_def {
            let def_id = internal(self.tcx, fn_def.def_id());

            // resolve an instance
            // let typing_env = rustc_middle::ty::TypingEnv::fully_monomorphized();
            let typing_env = rustc_middle::ty::TypingEnv::post_analysis(self.tcx, def_id);

            let args = internal(self.tcx, args);
            if let Ok(args) = self.tcx.try_normalize_erasing_regions(typing_env, args)
                && let Ok(Some(instance)) =
                    rustc_middle::ty::Instance::try_resolve(self.tcx, typing_env, def_id, args)
            {
                v_fn_info.push(new_info(instance, self.tcx));
            }
        }
        v_fn_info
    }
}
