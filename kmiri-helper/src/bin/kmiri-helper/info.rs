use kmiri_helper::FunctionInstanceInfo;
use rustc_data_structures::fx::FxHashSet;
use rustc_middle::ty::TyCtxt;
use rustc_public::{
    CrateDef,
    mir::{MirVisitor, mono::Instance, visit::Location},
    rustc_internal::internal,
    ty::{FnDef, GenericArgs, RigidTy, Ty},
};

fn new_info(instance: Instance) -> FunctionInstanceInfo {
    let name = instance.name();
    let Some(body) = instance.body() else {
        return FunctionInstanceInfo {
            instance: name,
            ..Default::default()
        };
    };
    let span = body.span;
    let lines = span.get_lines();
    FunctionInstanceInfo {
        instance: name,
        source_file: span.get_filename(),
        line_start: lines.start_line.try_into().unwrap(),
        line_end: lines.end_line.try_into().unwrap(),
    }
}

pub struct CollectInstance<'tcx> {
    pub fn_def: FxHashSet<(FnDef, GenericArgs)>,
    pub tcx: TyCtxt<'tcx>,
}

impl MirVisitor for CollectInstance<'_> {
    fn visit_ty(&mut self, ty: &Ty, _: Location) {
        if let Some(RigidTy::FnDef(fn_def, args)) = ty.kind().rigid()
        // && let Ok(instance) = Instance::resolve(*fn_def, args)
        {
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
            // resolve an instance
            // let typing_env = rustc_middle::ty::TypingEnv::fully_monomorphized();

            let def_id = internal(self.tcx, fn_def.def_id());
            let typing_env = rustc_middle::ty::TypingEnv::post_analysis(self.tcx, def_id);
            let args = internal(self.tcx, args);
            if let Ok(args) = self.tcx.try_normalize_erasing_regions(typing_env, args) {
                _ = rustc_middle::ty::Instance::try_resolve(self.tcx, typing_env, def_id, args);
            }

            // if let Ok(instance) = Instance::resolve(fn_def, &args) {
            //     v_fn_info.push(new_info(instance));
            // }
        }
        v_fn_info
    }
}
