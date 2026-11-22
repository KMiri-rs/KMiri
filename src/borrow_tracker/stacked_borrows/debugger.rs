use std::cell::RefCell;
use std::ops::Range;
use std::sync::Arc;

use ratatui::prelude::{Span as RatatuiSpan, *};
use ratatui::widgets::*;
use rustc_abi::{Align, Size};
use rustc_const_eval::interpret::AllocInfo;
use rustc_data_structures::fx::FxHashMap;
use rustc_span::Span;

use super::diagnostics::RetagCause;
use crate::borrow_tracker::stacked_borrows::diagnostics::RetagInfo;
use crate::debugger::state::{AllocInfo as DebuggerAllocInfo, RenderSrc};
use crate::debugger::utils::*;
use crate::*;

#[derive(Clone, Debug)]
pub struct DebuggerBorrowStacks {
    pub whole: DebuggerWholeAllocation,
    pub segments: Vec<DebuggerSegment>,
    pub span: FxHashMap<u64, Arc<DebuggerSpan>>,
}

impl DebuggerBorrowStacks {
    pub fn new() -> Self {
        let (segments, span) = Default::default();
        DebuggerBorrowStacks {
            whole: DebuggerWholeAllocation {
                // Safety: AllocId is a mere usize with PhantomData.
                alloc_id: AllocId(1.try_into().unwrap()),
                info: AllocInfo {
                    size: Size::ZERO,
                    align: Align::ONE,
                    kind: AllocKind::Dead,
                    mutbl: rustc_ast::Mutability::Not,
                },
            },
            segments,
            span,
        }
    }

    pub fn header() -> (Row<'static>, [Constraint; 15]) {
        let row = Row::new(vec![
            cell_left("AllocId"),
            cell_right("BasePaddr"), // info
            cell_right("Kind"),      // info
            cell_right("Names"),     // info
            cell_right("Bytes"),
            cell_right("Align"),
            cell_right("PosStart"),
            cell_right("PosEnd"),
            cell_right("BorTagID"),
            cell_right("StackIdx"),
            cell_right("Permission"),
            cell_right("Protected"),
            cell_right("Exposed"),
            cell_right("PrevTagID"),
            cell_right("RetagInfo"),
        ]);
        let widths = [
            Constraint::Min(8),  // AllocId
            Constraint::Min(9),  // BasePaddr
            Constraint::Min(5),  // Kind
            Constraint::Min(10), // Names
            Constraint::Min(4),  // Bytes
            Constraint::Min(4),  // Align
            Constraint::Min(8),  // PosStart
            Constraint::Min(8),  // PosEnd
            Constraint::Min(10), // BorTagID
            Constraint::Min(8),  // StackIdx
            Constraint::Min(15), // Permission
            Constraint::Min(4),  // Protected
            Constraint::Min(4),  // Exposed
            Constraint::Min(9),  // PrevTagID
            Constraint::Min(10), // RetagInfo
        ];
        (row, widths)
    }

    pub fn to_table_rows(
        &self,
        info: &DebuggerAllocInfo,
        iidx: &mut InverseIdx,
        mut inverse_idx: impl FnMut(InverseIdx),
    ) -> Vec<Row<'static>> {
        let mut level1 = true;
        let mut level2 = true;
        let mut rows = Vec::with_capacity(128);

        for (x, seg) in self.segments.iter().enumerate() {
            iidx.bs_segment = x;
            let whole = &self.whole;
            let Range { start, end } = seg.range;

            for (idx, item) in seg.stack.iter().rev().enumerate() {
                // `stack.get()` uses the underlying 0-based index, while the table is rendered
                // from top to bottom via `rev()`. Translate the visible row back to the actual
                // stack index here.
                iidx.bs_stack = seg.stack.len() - idx - 1;
                inverse_idx(*iidx);

                let idx = Text::from(idx.to_string()).style(Color::DarkGray);
                let permission = match item.permission {
                    Permission::Unique => Text::from("Unique").style(Color::Blue),
                    Permission::SharedReadWrite => Text::from("SharedReadWrite").style(Color::Cyan),
                    Permission::SharedReadOnly =>
                        Text::from("SharedReadOnly").style(Color::LightMagenta),
                    Permission::Disabled => Text::from("Disabled").style(Color::Red),
                };
                let (prev_id, retag_info) = item
                    .prev_tag
                    .as_ref()
                    .map(|p| (p.prev_tag(), p.retag_info()))
                    .unwrap_or_default();
                let row = Row::new(vec![
                    if level1 { cell_left(whole.alloc_id.0) } else { empty_cell() }, // AllocId
                    level_cell(level1, || {
                        info.base_addr.map(|addr| format!("0x{addr:x}")).unwrap_or_default()
                    }), // BasePaddr
                    level_cell(level1, || info.kind.map(kind_str).unwrap_or_default()), // Kind
                    level_cell(level1, || {
                        std::iter::empty()
                            .chain(&info.global)
                            .chain(&info.locals)
                            .map(String::from)
                            .collect::<Vec<String>>()
                            .join(",")
                    }), // Names
                    level_cell(level1, || hsize(whole.info.size.bytes())),           // Bytes
                    level_cell(level1, || hsize(whole.info.align.bytes())),          // Align
                    level_cell(level2, || start),                                    // PosStart
                    level_cell(level2, || end),                                      // PosEnd
                    cell_right(item.bor_tag_id),                                     // BorTagID
                    Cell::new(idx.right_aligned()),                                  // StackIdx
                    Cell::new(permission.right_aligned()),                           // Permission
                    cell_right(if item.protected { "✅" } else { "" }),              // Protected
                    cell_right(if item.prov_exposed { "✅" } else { "" }),           // Exposed
                    prev_id,                                                         // PrevTagID
                    retag_info,                                                      // RetagInfo
                ]);
                rows.push(row);

                level1 = false;
                level2 = false;
            }
            level2 = true;
        }

        rows
    }
}

fn cell_right(val: impl ToString) -> Cell<'static> {
    Cell::new(Text::from(val.to_string()).right_aligned())
}

fn cell_left(val: impl ToString) -> Cell<'static> {
    Cell::new(Text::from(val.to_string()))
}

fn empty_cell() -> Cell<'static> {
    Cell::new("")
}

fn level_cell<T: ToString>(level: bool, val: impl FnOnce() -> T) -> Cell<'static> {
    if level { cell_right((val()).to_string()) } else { empty_cell() }
}

#[derive(Clone, Debug)]
pub struct DebuggerWholeAllocation {
    pub alloc_id: AllocId,
    pub info: AllocInfo,
}

#[derive(Clone, Debug)]
pub struct DebuggerSegment {
    pub range: Range<u64>,
    pub stack: Vec<DebuggerBorrowStackItem>,
}

#[derive(Clone, Debug)]
pub struct DebuggerBorrowStackItem {
    pub bor_tag_id: u64,
    pub permission: Permission,
    pub protected: bool,
    pub prov_exposed: bool,
    pub prev_tag: Option<DebuggerPrevTag>,
}

#[derive(Clone, Debug)]
pub struct DebuggerPrevTag {
    /// This is usually a normal non-zero bor_tag_id.
    /// But for wildcard provenance, the id here is intentionallly 0 to render `*`.
    /// NOTE: prev_tag can be None, which renders an empty string.
    pub id: u64,
    pub retag_info: RetagInfo,
}

impl DebuggerPrevTag {
    fn prev_tag(&self) -> Cell<'static> {
        if self.id == 0 { cell_right("*") } else { cell_right(self.id) }
    }

    fn retag_info(&self) -> Cell<'static> {
        let cause = match self.retag_info.cause {
            RetagCause::Normal => "Normal",
            RetagCause::InPlaceFnPassing => "InPlaceFnPassing",
            RetagCause::FnEntry => "FnEntry",
            RetagCause::TwoPhase => "TwoPhase",
        };
        cell_right(cause)
    }
}

#[derive(Clone, Debug)]
pub struct DebuggerSpan {
    pub fn_name: String,
    pub source_file: String,
    pub source_file_hi: String,
    pub body_span: Span,
    pub body_line_start: u16,
    pub highlighted_span: Span,
    pub highlighted_line_start: u16,
    pub highlighted_line_end: u16,
    pub src: RenderSrc,
}

impl DebuggerSpan {
    pub fn new(
        fn_name: String,
        body_span: Span,
        highlighted_span: Span,
        ecx: &MiriInterpCx<'_>,
    ) -> Arc<Self> {
        #[derive(Debug, Hash, PartialEq, Eq)]
        struct Key {
            body_span: Span,
            highlighted_span: Span,
            fn_name: String,
        }
        thread_local! {
            static CACHE: RefCell<FxHashMap<Key, Arc<DebuggerSpan>>> = Default::default();
        }

        fn inner(
            fn_name: String,
            body_span: Span,
            highlighted_span: Span,
            ecx: &MiriInterpCx<'_>,
        ) -> Arc<DebuggerSpan> {
            let tcx = ecx.tcx.tcx;
            let sm = tcx.sess.source_map();
            let body_span = body_span.source_callsite();
            let body_line_start = line_nr(sm, body_span.lo());
            let highlighted_span = highlighted_span.source_callsite();
            let highlighted_line_start = line_nr(sm, highlighted_span.lo());
            let highlighted_line_end = line_nr(sm, highlighted_span.hi());
            let source_file_hi = source_file(sm, highlighted_span);
            let source_file = source_file(sm, body_span);
            DebuggerSpan {
                src: if body_span.is_dummy() || source_file != source_file_hi {
                    RenderSrc {
                        lines: Text::from(format!(
                            "fn_name: {fn_name}\nbody span: {body_span:?}\nspan: {highlighted_span:?}"
                        ))
                        .lines,
                        ..RenderSrc::default()
                    }
                } else {
                    render_src(body_span, highlighted_span, sm)
                },
                fn_name,
                source_file,
                source_file_hi,
                body_span,
                body_line_start,
                highlighted_span,
                highlighted_line_start,
                highlighted_line_end,
            }
            .into()
        }

        CACHE.with_borrow_mut(move |map| {
            let key = Key { body_span, highlighted_span, fn_name };
            if let Some(val) = map.get(&key) {
                val.clone()
            } else {
                let val = inner(key.fn_name.clone(), body_span, highlighted_span, ecx);
                map.insert(key, val.clone());
                val
            }
        })
    }

    pub fn title(&self) -> Line<'static> {
        let location = format!(
            " - {}:{}..{}",
            self.source_file, self.highlighted_line_start, self.highlighted_line_end
        );
        vec![
            RatatuiSpan::styled("[Source] ", Style::from(Color::DarkGray)),
            RatatuiSpan::raw(self.fn_name.clone()),
            RatatuiSpan::styled(location, Style::from(Color::DarkGray)),
        ]
        .into()
    }

    pub fn span_different(&self) -> bool {
        self.source_file != self.source_file_hi
    }
}

fn line_nr(sm: &rustc_span::source_map::SourceMap, pos: rustc_span::BytePos) -> u16 {
    u16::try_from(sm.lookup_char_pos(pos).line).unwrap_or(0)
}
