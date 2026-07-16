use kmiri_helper::FunctionInstanceInfo;
use rustc_middle::ty::TyCtxt;
use rustc_middle::ty::print::{with_no_trimmed_paths, with_resolve_crate_name};
use rustc_public::CrateDef;
use rustc_public::mir::mono::Instance;
use rustc_public::rustc_internal::internal;
use rustc_span::{BytePos, FileName, RealFileName, Span, source_map::SourceMap};

pub fn new<'tcx>(instance: Instance, tcx: TyCtxt<'tcx>) -> FunctionInstanceInfo {
    let def_id = internal(tcx, instance.def.def_id());
    let name = with_no_trimmed_paths!(with_resolve_crate_name!(tcx.def_path_str(def_id)));

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
