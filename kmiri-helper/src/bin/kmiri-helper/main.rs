#![feature(rustc_private)]

use itertools::Itertools;
use rustc_middle::ty::TyCtxt;
use rustc_public::{local_crate, mir::mono::MonoItem, rustc_internal::internal};
use std::{ops::ControlFlow, path::Path};

extern crate indexmap;
extern crate rustc_data_structures;
extern crate rustc_driver;
extern crate rustc_hir;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_public;
extern crate rustc_session;
extern crate rustc_span;

mod coercion;
mod info;
mod reachability;

fn main() {
    let args: Box<[_]> = std::env::args().collect();
    rustc_public::run_with_tcx!(&args, analysis).unwrap();
}

fn analysis(tcx: TyCtxt) -> ControlFlow<()> {
    let krate = local_crate();

    let (v_mono_item, _callgraph) = reachability::collect(tcx, &krate.name);
    let v_fn: Vec<_> = v_mono_item
        .into_iter()
        .filter_map(|mono| {
            if let MonoItem::Fn(instance) = mono {
                Some(info::new(instance, tcx))
            } else {
                None
            }
        })
        .sorted_unstable()
        .dedup()
        .collect();
    println!("{}: collected {}", krate.name, v_fn.len());

    let dir_target = std::env::var(kmiri_helper::ENV__KMIRI_DIR_TARGET).unwrap();
    let dir_analysis = Path::new(&dir_target).join(kmiri_helper::ANALYSIS);

    // Use stable_crate_id instead of tcx.crate_hash() to avoid triggering HIR hash computation.
    // tcx.crate_hash() requires calling opt_hash.unwrap() on OwnerInfo, which panics when needs_hir_hash() is false.
    // Instead we use StableCrateId which can be obtained without accessing HIR owners.
    let stable_id = tcx.stable_crate_id(internal(tcx, krate.id)).as_u64();
    let json_path = dir_analysis.join(format!("{}_{stable_id}.json", krate.name,));
    if let Err(err) = std::fs::create_dir_all(&dir_analysis) {
        panic!("Failed to create analysis directory {dir_analysis:?}: {err:?}");
    }
    let file = std::fs::File::create(dbg!(json_path)).unwrap();
    serde_json::to_writer_pretty(file, &v_fn).unwrap();

    ControlFlow::Continue(())
}
