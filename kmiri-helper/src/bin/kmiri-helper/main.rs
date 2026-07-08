#![feature(rustc_private)]

use kmiri_helper::*;
use rustc_middle::{mir::visit::Visitor, ty::TyCtxt};
use rustc_public::{CrateDef, local_crate};
use std::{ops::ControlFlow, path::Path};

extern crate rustc_data_structures;
extern crate rustc_driver;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_public;
extern crate rustc_span;

mod info;

fn main() {
    let args: Box<[_]> = std::env::args().collect();
    rustc_public::run_with_tcx!(&args, analysis).unwrap();
}

fn analysis(tcx: TyCtxt) -> ControlFlow<()> {
    let krate = local_crate();
    dbg!(&krate.name);
    let fn_defs = krate.fn_defs();

    let mut collector = info::CollectInstance::new(tcx);
    for fn_def in fn_defs {
        // Start from all non-generic functions to find monomorphized instances.
        let def_id = rustc_public::rustc_internal::internal(tcx, fn_def.def_id());
        let generics = tcx.generics_of(def_id);
        if !generics.requires_monomorphization(tcx) {
            if let Some(local_def_id) = def_id.as_local()
                && tcx.hir_maybe_body_owned_by(local_def_id).is_none()
            {
                continue;
            }
            let body = tcx.instance_mir(rustc_middle::ty::InstanceKind::Item(def_id));
            collector.visit_body(body);
        }
    }
    let mut v_fn = collector.into_info();
    dedup(&mut v_fn);

    let dir_target = std::env::var(kmiri_helper::ENV_INNER_DIR_TARGET).unwrap();
    let dir_analysis = Path::new(&dir_target).join(kmiri_helper::ANALYSIS);
    _ = std::fs::create_dir(dbg!(&dir_analysis));
    let json_path = dir_analysis.join(format!("{}.json", krate.name));
    let file = std::fs::File::create(dbg!(json_path)).unwrap();
    serde_json::to_writer_pretty(file, &v_fn).unwrap();

    ControlFlow::Continue(())
}
