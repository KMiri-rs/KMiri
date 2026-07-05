#![feature(rustc_private)]

use std::ops::ControlFlow;

use rustc_middle::ty::TyCtxt;
use rustc_public::{local_crate, mir::MirVisitor};

extern crate rustc_data_structures;
extern crate rustc_driver;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_public;

mod info;

fn main() {
    let args: Box<[_]> = std::env::args().collect();
    rustc_public::run_with_tcx!(&args, analysis).unwrap();
}

fn analysis(_tcx: TyCtxt) -> ControlFlow<()> {
    let krate = local_crate();
    dbg!(&krate.name);
    let fn_defs = krate.fn_defs();

    let mut collector = info::CollectInstance::default();
    for fn_def in fn_defs {
        if let Some(body) = fn_def.body() {
            collector.visit_body(&body);
        }
    }
    let mut v_fn = collector.into_info();
    v_fn.sort_unstable();

    let dir = std::env::var("DIR_ANALYSIS").unwrap();
    _ = std::fs::create_dir(&dir);
    let file = std::fs::File::create(format!("{dir}/{}.json", krate.name)).unwrap();
    serde_json::to_writer_pretty(file, &v_fn).unwrap();

    ControlFlow::Continue(())
}
