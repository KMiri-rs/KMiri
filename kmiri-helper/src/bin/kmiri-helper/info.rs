use kmiri_helper::FunctionInstanceInfo;
use rustc_data_structures::fx::FxHashSet;
use rustc_public::{
    mir::{MirVisitor, mono::Instance, visit::Location},
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

#[derive(Default)]
pub struct CollectInstance {
    pub fn_def: FxHashSet<(FnDef, GenericArgs)>,
    pub info: Vec<FunctionInstanceInfo>,
}

impl MirVisitor for CollectInstance {
    fn visit_ty(&mut self, ty: &Ty, _: Location) {
        if let Some(RigidTy::FnDef(fn_def, args)) = ty.kind().rigid()
        // && let Ok(instance) = Instance::resolve(*fn_def, args)
        {
            self.fn_def.insert((*fn_def, args.clone()));
            // self.info.push(new_info(instance));
        }
        self.super_ty(ty);
    }
}

impl CollectInstance {
    pub fn into_info(self) -> Vec<FunctionInstanceInfo> {
        let mut v_fn_info = Vec::with_capacity(dbg!(self.fn_def.len()));
        for (fn_def, args) in self.fn_def {
            if let Ok(instance) = Instance::resolve(fn_def, &args) {
                v_fn_info.push(new_info(instance));
            }
        }
        v_fn_info
    }
}
