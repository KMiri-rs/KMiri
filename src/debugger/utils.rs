use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};

use ratatui::text::{Line, Span as RatatuiSpan};
use rustc_data_structures::fx::FxHashMap;
use rustc_hir::def_id::DefId;
use rustc_middle::ty::{Instance, TyCtxt};
use rustc_span::source_map::SourceMap;
use rustc_span::{FileName, Pos, RealFileName, RemapPathScopeComponents, Span};

use crate::concurrency::thread::EvalContextExt;
use crate::debugger::state::RenderSrc;
use crate::debugger::tui::theme::STYLE_HIGHTLIGHTED;
use crate::helpers::ToUsize;
use crate::{MemoryKind, MiriInterpCx, MiriMemoryKind};

pub fn kind_str(kind: MemoryKind) -> &'static str {
    match kind {
        MemoryKind::Stack => "Stack",
        MemoryKind::CallerLocation => "CallerLoc",
        MemoryKind::Machine(kind) =>
            match kind {
                MiriMemoryKind::Kernel => "Kernel",
                MiriMemoryKind::Pool => "Pool",
                MiriMemoryKind::Rust => "Rust",
                MiriMemoryKind::Miri => "Miri",
                MiriMemoryKind::C => "C",
                MiriMemoryKind::WinHeap => "WinHeap",
                MiriMemoryKind::WinLocal => "WinLocal",
                MiriMemoryKind::Machine => "Machine",
                MiriMemoryKind::Runtime => "Runtime",
                MiriMemoryKind::Global => "Global",
                MiriMemoryKind::ExternStatic => "ExternStatic",
                MiriMemoryKind::Tls => "Tls",
                MiriMemoryKind::Mmap => "Mmap",
                MiriMemoryKind::SocketAddress => "SocketAddress",
            },
    }
}

pub fn hsize(n: impl humansize::ToF64 + humansize::Unsigned) -> String {
    humansize::format_size(n, humansize::BINARY)
}

#[expect(unused)]
pub fn find_def_id_from_span(tcx: TyCtxt<'_>, span: Span) -> Option<(Span, DefId)> {
    let mut best: Option<(Span, DefId)> = None;

    for local_def_id in tcx.hir_crate_items(()).definitions() {
        let def_id = local_def_id.to_def_id();
        let Some(def_span) = tcx.hir_span_if_local(def_id) else {
            continue;
        };

        if def_span.contains(span) {
            match best {
                None => best = Some((def_span, def_id)),
                Some((best_span, _)) => {
                    // 选更“内层”的那个定义
                    if best_span.contains(def_span) && !def_span.contains(best_span) {
                        best = Some((def_span, def_id));
                    }
                }
            }
        }
    }

    best
}

pub fn source_file(sm: &SourceMap, span: Span) -> String {
    // Force path remapping, because `prefer_remapped_unconditionally` doesn't always work.
    // Use `--remap-path-prefix` to shorten the long sysroot path, e.g.
    // ./miri run tests/pass/debugger_test.rs --debugger --remap-path-prefix=$(rustc --print=sysroot)/lib/rustlib/src/rust/library/=
    match sm.span_to_filename(span) {
        FileName::Real(path) if let Some(local_path) = path.clone().into_local_path() =>
            FileName::Real(sm.path_mapping().to_real_filename(&RealFileName::empty(), local_path)),
        file_name => file_name,
    }
    .prefer_remapped_unconditionally()
    .to_string()
}

/// `sm.span_to_snippet` fails when the `SourceFile` was imported from another
/// crate and `--remap-path-prefix` removed the local path (`local: None`).
/// In that case `ensure_source_file_source_present` cannot reconstruct the
/// path and freezes `external_src` as `AbsentErr`.  We work around it by
/// reading the file directly via `embeddable_name`, which always contains the
/// absolute on-disk path.
///
/// Strategy:
/// 1. Try to inject the file content into `SourceFile::external_src` via
///    `add_external_src` *before* calling `span_to_snippet`.  If the
///    content hash matches, the source is cached inside the `SourceFile`
///    itself and all future calls on the same file succeed without extra I/O.
/// 2. If that fails (hash mismatch, or `external_src` already frozen as
///    `AbsentErr` by a prior `span_to_snippet` call), fall back to reading
///    the file directly with our own `PathBuf`-keyed cache.
fn span_to_snippet_with_fallback(sm: &SourceMap, span: Span) -> Result<String, String> {
    let sf = sm.lookup_source_file(span.lo());
    if let FileName::Real(ref real) = sf.name {
        let (_, abs_path) = real.embeddable_name(RemapPathScopeComponents::DIAGNOSTICS);
        sf.add_external_src(|| fs::read_to_string(abs_path).ok());
    }

    if let Ok(s) = sm.span_to_snippet(span) {
        return Ok(s);
    }

    // add_external_src did not help (hash mismatch or already AbsentErr).
    cache_source_file(span, &sf)
}

// Fall back to reading directly, with a global path-keyed cache.
fn cache_source_file(span: Span, sf: &rustc_span::SourceFile) -> Result<String, String> {
    static CACHE: LazyLock<Mutex<FxHashMap<PathBuf, Arc<str>>>> = LazyLock::new(Default::default);

    let FileName::Real(ref real) = sf.name else {
        return Err(format!("non-real filename: {:?}", sf.name));
    };
    let (_, abs_path) = real.embeddable_name(RemapPathScopeComponents::DIAGNOSTICS);
    let full_src = {
        let mut cache = CACHE.lock().unwrap();
        if let Some(src) = cache.get(abs_path) {
            Arc::clone(src)
        } else {
            let src = fs::read_to_string(abs_path).map_err(|e| format!("{e}: {abs_path:?}"))?;
            let src = Arc::from(src);
            cache.insert(abs_path.to_owned(), Arc::clone(&src));
            src
        }
    };

    let start = (span.lo() - sf.start_pos).to_usize();
    let end = (span.hi() - sf.start_pos).to_usize();
    full_src.get(start..end).map(str::to_owned).ok_or_else(|| {
        format!("span [{start}..{end}] out of range for file len {}", full_src.len())
    })
}

pub fn render_src(
    body_span: rustc_span::Span,
    highlight_span: rustc_span::Span,
    sm: &SourceMap,
) -> RenderSrc {
    let source_text = match span_to_snippet_with_fallback(sm, body_span) {
        Ok(source_text) => source_text,
        Err(err) => {
            log!("render_src: {err}");
            return RenderSrc {
                lines: vec![
                    "Could not load source snippet:".into(),
                    format!("{err}").into(),
                    "body_span".into(),
                    format!("  ={body_span:?}").into(),
                ],
                highlighted_idx: None,
            };
        }
    };

    // Get the absolute byte positions for relative calculations
    let body_lo = body_span.lo();
    let highlight_lo = highlight_span.lo();
    let highlight_hi = highlight_span.hi();

    let mut render_src = RenderSrc::default();
    let lines = &mut render_src.lines;

    let mut current_pos = body_lo;

    // Split the source by lines and construct Ratatui Line/Span structures.
    // Use split_inclusive to get the terminator length — lines() strips \r\n
    // but we need to advance current_pos by the full byte count including \r.
    for (idx, raw_line) in source_text.split('\n').enumerate() {
        let (text_line, terminator_len) =
            raw_line.strip_suffix('\r').map(|text_line| (text_line, 2)).unwrap_or((raw_line, 1));
        let line_len = text_line.len().try_into().unwrap();
        let line_end = current_pos + rustc_span::BytePos(line_len);

        let mut line_spans = Vec::new();

        // Check if the current line intersects with the highlight_span
        // Case A: The line is entirely before the highlight
        // Case B: The line is entirely after the highlight
        if line_end <= highlight_lo || current_pos >= highlight_hi {
            // No intersection: add as plain text
            line_spans.push(RatatuiSpan::raw(text_line.to_string()));
        } else {
            // Intersection exists: split the line into parts
            let line_start_pos = current_pos;

            // Calculate relative start and end indices for the highlight within this specific line
            let h_start_in_line = if highlight_lo > line_start_pos {
                highlight_lo.0.checked_sub(line_start_pos.0).unwrap().to_usize()
            } else {
                0
            };

            let h_end_in_line = if highlight_hi < line_end {
                highlight_hi.0.checked_sub(line_start_pos.0).unwrap().to_usize()
            } else {
                text_line.len()
            };

            // Snap to char boundaries: rustc BytePos counts raw bytes (incl. \r),
            // so drift from CRLF line endings can land offsets mid-character.
            let h_start_in_line = text_line.floor_char_boundary(h_start_in_line);
            let h_end_in_line = text_line.ceil_char_boundary(h_end_in_line);

            if h_start_in_line > 0 {
                line_spans.push(RatatuiSpan::raw(text_line[..h_start_in_line].to_string()));
            }

            line_spans.push(RatatuiSpan::styled(
                text_line[h_start_in_line..h_end_in_line].to_string(),
                STYLE_HIGHTLIGHTED,
            ));

            if h_end_in_line < text_line.len() {
                line_spans.push(RatatuiSpan::raw(text_line[h_end_in_line..].to_string()));
            }

            // Update highlighted_idx
            let idx = u16::try_from(idx).unwrap();
            render_src.highlighted_idx = Some(match render_src.highlighted_idx {
                Some([start, end]) => [start.min(idx), end.max(idx)],
                None => [idx; 2],
            });
        }

        lines.push(Line::from(line_spans));

        current_pos = line_end + rustc_span::BytePos(terminator_len);
    }
    render_src
}

#[derive(Default, Debug, Clone, Copy)]
pub struct InverseIdx {
    pub alloc: usize,
    pub bs_segment: usize,
    pub bs_stack: usize,
}

/// Given the highlighted_idx and area height, center the view by returning the start row idx.
pub fn src_view_centering(highlighted_idx: [u16; 2], height: u16) -> u16 {
    let [start_idx, end_idx] = highlighted_idx;
    let [start_idx, end_idx]: [usize; 2] = [start_idx.into(), end_idx.into()];

    let height: usize = height.into();
    let scroll = if end_idx + 2 < height {
        // The highlighted lines fit into current view from first line.
        0
    } else {
        // Pan the view to the first highlighted line.
        let gap = height.checked_sub(1 + end_idx - start_idx).unwrap_or(height) / 2;
        start_idx.saturating_sub(gap)
    };
    scroll.try_into().unwrap()
}

pub fn pos_to_line_nr(sm: &SourceMap, pos: rustc_span::BytePos) -> u16 {
    let loc = sm.lookup_char_pos(pos);
    u16::try_from(loc.line).unwrap_or(0)
}

pub fn instance_name<'tcx>(ecx: &MiriInterpCx<'tcx>, instance: Instance<'tcx>) -> String {
    static RECORDED: LazyLock<Mutex<FxHashMap<DefId, String>>> = LazyLock::new(Default::default);

    let mut recorded = RECORDED.lock().unwrap();
    recorded
        .entry(instance.def_id())
        .or_insert_with(|| kmiri_helper::instance_name(ecx.tcx.tcx, instance))
        .clone()
}

/// Checks if the current frame matches `src_line` (e.g., "path/to/file.rs:10" or "path/to/file.rs").
///
/// Matches the name part against `frame.fn_name` and the optional line number against `frame.line_start`.
pub fn is_src_line_reached(ecx: &MiriInterpCx<'_>, src_line: &str) -> bool {
    let Some(frame) = ecx.active_thread_stack().last() else {
        return true;
    };

    let sm = ecx.tcx.sess.source_map();
    let span = frame.current_span();
    let file_name = source_file(sm, span);
    let line_start = pos_to_line_nr(sm, span.lo());

    let Some((file, line)) = src_line.rsplit_once(':') else {
        return src_line == file_name;
    };
    if file != file_name {
        return false;
    }
    line.parse::<u16>().map(|line| line == line_start).unwrap_or(false)
}
