use ratatui::text::Line;
use rustc_data_structures::either::Either;
use rustc_data_structures::fx::{FxHashMap, FxHashSet};
use rustc_hir::def_id::DefId;
use rustc_middle::mir::interpret::GlobalAlloc;
use rustc_middle::mir::{self, BasicBlockData};

use crate::borrow_tracker::stacked_borrows::debugger::DebuggerBorrowStacks;
use crate::debugger::format_local::format_local;
use crate::debugger::utils::{instance_name, pos_to_line_nr, render_src, source_file};
use crate::debugger::{debugger_log, reachability};
use crate::*;

#[derive(Clone, Debug)]
pub struct FrameInfo {
    pub fn_name: String,
    pub source_file: String,
    pub line_start: u16,
    pub line_end: u16,
    pub locals: Vec<LocalInfo>,
}

impl FrameInfo {
    pub fn display_src_line(&self) -> String {
        let file = &self.source_file;
        let start = self.line_start;
        let end = self.line_end;
        if start == end { format!("{file}:{start}") } else { format!("{file}:{start}:{end}") }
    }
}

#[derive(Clone, Debug)]
pub struct CurrentLocation {
    /// Full source code with current location highlighted.
    pub render_src: RenderSrc,
    /// The line number of start and end in source file.
    pub line_start: u16,
    pub line_end: u16,
    /// A basic block with current location highlighted.
    pub render_mir: Vec<String>,
    pub render_mir_highlighted_idx: u16,
}

#[derive(Clone, Debug, Default)]
pub struct RenderSrc {
    /// Full source code with current location highlighted.
    pub lines: Vec<Line<'static>>,
    pub highlighted_idx: Option<[u16; 2]>,
}

type Ptr = interpret::Pointer<crate::Provenance>;

#[derive(Clone, Debug)]
pub struct LocalInfo {
    pub idx: String,
    pub name: String,
    pub value: String,
    pub ty: String,
    pub state: LocalKind,
    pub alloc_id: Option<AllocId>,
    pub ptr: Option<Ptr>,
}

impl LocalInfo {
    pub fn queried(&self, query: Option<u64>) -> bool {
        query.is_none()
            || query == self.alloc_id.map(|id| id.0.get())
            || self.ptr.map(|ptr| ptr.into_raw_parts().1.bytes()) == query
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalKind {
    Initialized,
    Pointer,
    Uninitialized,
    Dead,
}

#[derive(Clone, Debug)]
pub struct CfgLine {
    pub block: usize,
    pub text: String,
    pub is_current: bool,
    pub successors: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct AllocInfo {
    pub alloc_id: AllocId,
    pub ptr: Option<usize>,
    pub base_addr: Option<u64>,
    pub dealloc: bool,
    pub kind: Option<MemoryKind>,
    /// Allocation size in bytes.
    pub size: Option<usize>,
    pub align: Option<u64>,
    pub provenance_exposed: bool,
    pub global: Option<String>,
    pub locals: Vec<String>,
    pub borrow_stacks: DebuggerBorrowStacks,
}

impl AllocInfo {
    /// Returns true if the query matches one of these fields: alloc_id, ptr and base_addr.
    pub fn queried(&self, query: Option<u64>) -> bool {
        query.is_none()
            || query == Some(self.alloc_id.0.get())
            || query == self.ptr.map(|ptr| ptr as u64)
            || query == self.base_addr
    }
}

#[derive(Clone, Debug)]
pub struct OutputSpan {
    pub is_stderr: bool,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct DebuggerState {
    pub current_thread: ThreadId,
    pub step_count: u64,
    /// The first frame is the current stack.
    pub stack_frames: Vec<FrameInfo>,
    pub function_instances: reachability::Reachability,
    pub current_location: CurrentLocation,
    pub cfg_lines: Vec<CfgLine>,
    pub locals: Vec<LocalInfo>,
    pub allocs: Vec<AllocInfo>,
    pub output: Vec<OutputSpan>,
    /// The lowest allocated stack vaddr.
    pub min_stack_ptr: Option<u64>,
    pub last_recorded_stack_ptr: Option<u64>,
    // Although this is a global state that won't change after initialization.
    pub borrow_tracker_method: Option<BorrowTrackerMethod>,
}

impl DebuggerState {
    pub fn capture<'tcx>(ecx: &MiriInterpCx<'tcx>) -> Self {
        // The last is the current stack.
        let stack = ecx.active_thread_stack();

        let min_stack_ptr = ecx
            .machine
            .alloc_addresses
            .borrow()
            .min_allocated_stack_paddr()
            .map(|(paddr, _)| paddr);
        let last_recorded_stack_ptr =
            ecx.machine.threads.active_thread_ref().stack_addr_records.last().map(|record| {
                let vaddr = record.addr as usize;
                let paddr_fallback = || {
                    mirch::try_kernel_code_vaddr_to_paddr(vaddr)
                        .unwrap_or_else(|| mirch::try_boot_pt_vaddr_to_paddr(vaddr).unwrap())
                };
                mirch::page_walk_or(vaddr, paddr_fallback).unwrap_or_else(paddr_fallback) as u64
            });

        // Reverse the stack frames: the first is the current.
        let stack_frames: Vec<_> =
            stack.iter().rev().map(|frame| capture_frame(ecx, frame)).collect();

        let current_location =
            stack.last().map(|frame| capture_location(ecx, frame)).unwrap_or_else(|| {
                CurrentLocation {
                    render_src: RenderSrc {
                        lines: vec!["No stack frame found.".into()],
                        highlighted_idx: None,
                    },
                    line_start: 0,
                    line_end: 0,
                    render_mir: vec!["No basic block found.".into()],
                    render_mir_highlighted_idx: 0,
                }
            });

        let locals = stack.last().map(|frame| capture_locals(ecx, frame)).unwrap_or_default();
        let cfg_lines = stack.last().map(capture_cfg_lines).unwrap_or_default();
        let allocs = capture_allocs(ecx, &locals);
        let function_instances = ecx.machine.reachable_function_instances.clone();
        let output = ecx
            .machine
            .debugger_output
            .borrow()
            .iter()
            .map(|(is_stderr, text)| OutputSpan { is_stderr: *is_stderr, text: text.clone() })
            .collect();

        Self {
            current_thread: ecx.active_thread(),
            step_count: ecx.machine.basic_block_count,
            stack_frames,
            function_instances,
            current_location,
            cfg_lines,
            locals,
            allocs,
            output,
            min_stack_ptr,
            last_recorded_stack_ptr,
            borrow_tracker_method: ecx
                .machine
                .borrow_tracker
                .as_ref()
                .map(|bt| bt.borrow().borrow_tracker_method()),
        }
    }

    pub fn current_stack_frame(&self) -> Option<&FrameInfo> {
        self.stack_frames.first()
    }
}

fn capture_locals<'tcx>(
    ecx: &MiriInterpCx<'tcx>,
    frame: &Frame<'tcx, Provenance, FrameExtra<'tcx>>,
) -> Vec<LocalInfo> {
    frame
        .locals()
        .iter()
        .enumerate()
        .map(|(idx, local)| {
            let local_idx = mir::Local::from_usize(idx);
            let body = frame.body();
            let local_decl = &body.local_decls[local_idx];
            let (value, kind) = format_local(ecx, frame, local_idx);
            let (alloc_id, ptr) = match local.as_mplace_or_imm_ghost() {
                Some(Either::Left((ptr, _))) =>
                    if let Some(prov) = ptr.provenance {
                        let ptr = Ptr::new(prov, ptr.addr());
                        (prov.get_alloc_id(), Some(ptr))
                    } else {
                        let ptr = Ptr::new(crate::Provenance::Wildcard, ptr.addr());
                        (None, Some(ptr))
                    },
                Some(Either::Right(imm)) => {
                    match imm {
                        Immediate::Scalar(Scalar::Ptr(ptr, _)) => {
                            let ptr: Ptr = ptr;
                            (ptr.provenance.get_alloc_id(), Some(ptr))
                        }
                        // FIXME: we only extract the alloc id of data pointer here, but miss
                        // the vtable ptr for dyn pointer here.
                        Immediate::ScalarPair(Scalar::Ptr(ptr, _), _) => {
                            let ptr: Ptr = ptr;
                            (ptr.provenance.get_alloc_id(), Some(ptr))
                        }
                        _ => (None, None),
                    }
                }
                None => (None, None),
            };
            LocalInfo {
                idx: format!("_{idx}"),
                name: find_name_for_local(body, local_idx)
                    .map(|x| x.to_string())
                    .unwrap_or_default(),
                value,
                ty: local_decl.ty.to_string(),
                state: kind,
                alloc_id,
                ptr,
            }
        })
        .collect()
}

pub fn find_name_for_local(body: &mir::Body<'_>, local: mir::Local) -> Option<rustc_span::Symbol> {
    body.var_debug_info.iter().find_map(|var_info| {
        if let mir::VarDebugInfoContents::Place(place) = var_info.value {
            if place.local == local && place.projection.as_slice().is_empty() {
                return Some(var_info.name);
            }
        }
        None
    })
}

fn capture_frame<'tcx>(
    ecx: &MiriInterpCx<'tcx>,
    frame: &Frame<'tcx, Provenance, FrameExtra<'tcx>>,
) -> FrameInfo {
    let sm = ecx.tcx.sess.source_map();
    let span = frame.current_span();
    FrameInfo {
        fn_name: instance_name(ecx, frame.instance()),
        source_file: source_file(sm, span),
        line_start: pos_to_line_nr(sm, span.lo()),
        line_end: pos_to_line_nr(sm, span.hi()),
        locals: capture_locals(ecx, frame),
    }
}

fn capture_cfg_lines(frame: &Frame<'_, Provenance, FrameExtra<'_>>) -> Vec<CfgLine> {
    let current_block = match frame.current_loc() {
        Either::Left(loc) => Some(loc.block.index()),
        Either::Right(_) => None,
    };

    frame
        .body()
        .basic_blocks
        .iter_enumerated()
        .map(|(bb, block_data)| {
            let bb_idx = bb.index();
            let mut text = format!("bb{bb_idx}");
            let mut successors = Vec::new();
            if let Some(term) = &block_data.terminator {
                let succs: Vec<usize> = term.successors().map(|target| target.index()).collect();
                successors = succs.clone();
                if succs.is_empty() {
                    text.push_str(" -> <end>");
                } else {
                    text.push_str(" -> ");
                    text.push_str(
                        &succs.iter().map(|s| format!("bb{s}")).collect::<Vec<_>>().join(", "),
                    );
                }
            }
            CfgLine { block: bb_idx, text, is_current: current_block == Some(bb_idx), successors }
        })
        .collect()
}

fn capture_allocs(ecx: &MiriInterpCx<'_>, locals: &[LocalInfo]) -> Vec<AllocInfo> {
    let mut entries = Vec::new();

    #[derive(Clone, Copy, Debug)]
    struct Local<'a> {
        name: &'a str,
        ptr: Option<Ptr>,
    }
    let mut map_locals: FxHashMap<AllocId, Vec<Local<'_>>> = FxHashMap::default();
    for local in locals {
        if let Some(id) = local.alloc_id {
            let name = if local.name.is_empty() { &*local.idx } else { &local.name };
            let ptr = local.ptr;
            let local = Local { name, ptr };
            map_locals.entry(id).and_modify(|v| v.push(local)).or_insert_with(|| vec![local]);
        }
    }
    fn split_locals(v: &[Local<'_>]) -> (Vec<String>, Option<usize>) {
        let names = v.iter().map(|local| local.name.to_owned()).collect();
        let set: FxHashSet<_> =
            v.iter().map(|local| local.ptr.map(|p| p.into_raw_parts().1.bytes_usize())).collect();
        if set.len() > 2 {
            debugger_log(format!("{v:?} has multiple pointer addrs: {set:?}"));
        }
        (names, set.iter().find_map(|p| *p))
    }

    let alloc_map = ecx.memory.alloc_map();
    let alloc_spans = ecx.machine.allocation_spans.borrow();

    let item_name = |did: DefId| ecx.tcx.item_name(did).as_str().to_owned();
    let ptr_meta = |alloc_id: AllocId| {
        if let Some((kind, allocation)) = alloc_map.get(alloc_id) {
            (Some(*kind), Some(allocation.len()), Some(allocation.align.bytes()))
        } else {
            Default::default()
        }
    };

    // states of stack or tree borrow checker
    let borrow_stacks = |alloc_id: AllocId| {
        if let Some(alloc_extra) = ecx.get_alloc_extra(alloc_id).discard_err()
            && let Some(state) = &alloc_extra.borrow_tracker
        {
            match state {
                borrow_tracker::AllocState::StackedBorrows(val) => val.borrow().debugger(ecx),
                borrow_tracker::AllocState::TreeBorrows(_) => DebuggerBorrowStacks::new(),
            }
        } else {
            DebuggerBorrowStacks::new()
        }
    };

    for (&alloc_id, (_alloc, dealloc)) in alloc_spans.iter() {
        let alloc_state = ecx.machine.alloc_addresses.borrow();
        let provenance_exposed = alloc_state.exposed.contains(&alloc_id);
        let base_addr = alloc_state.base_paddr.get(&alloc_id).copied();
        // get_alloc_extra also requires alloc_state, so end the Ref borrow here
        drop(alloc_state);
        let global = ecx.tcx.try_get_global_alloc(alloc_id).and_then(|ga| {
            Some(match ga {
                GlobalAlloc::Function { instance } => item_name(instance.def_id()),
                GlobalAlloc::Static(did) => item_name(did),
                _ => return None,
            })
        });
        let (kind, size, align) = ptr_meta(alloc_id);
        let (names, ptr) = map_locals.get(&alloc_id).map(|v| split_locals(v)).unwrap_or_default();

        entries.push(AllocInfo {
            alloc_id,
            ptr,
            base_addr,
            dealloc: dealloc.is_some(),
            kind,
            size,
            align,
            provenance_exposed,
            global,
            locals: names,
            borrow_stacks: borrow_stacks(alloc_id),
        });
    }

    // Remaining locals' allocation.
    let set_id: FxHashSet<_> = entries.iter().map(|e| e.alloc_id).collect();
    for (alloc_id, v_locals) in map_locals.into_iter().filter(|(id, _)| !set_id.contains(id)) {
        let info = ecx.get_alloc_info(alloc_id);
        let (names, ptr) = split_locals(&v_locals);
        let alloc_state = ecx.machine.alloc_addresses.borrow();
        let provenance_exposed = alloc_state.exposed.contains(&alloc_id);
        let base_addr = alloc_state.base_paddr.get(&alloc_id).copied();
        // get_alloc_extra also requires alloc_state, so end the Ref borrow here
        drop(alloc_state);
        let borrow_stacks = borrow_stacks(alloc_id);
        if let Some(ga) = ecx.tcx.try_get_global_alloc(alloc_id) {
            let global = match ga {
                GlobalAlloc::Function { instance } => item_name(instance.def_id()),
                GlobalAlloc::Static(did) => item_name(did),
                GlobalAlloc::VTable(..) => "Vtable".to_owned(),
                GlobalAlloc::Memory(_) => {
                    let (_, size, align) = ptr_meta(alloc_id);
                    entries.push(AllocInfo {
                        alloc_id,
                        ptr,
                        base_addr,
                        dealloc: false,
                        kind: Some(MiriMemoryKind::Global.into()),
                        size,
                        align,
                        provenance_exposed: false,
                        global: Some("Const".to_owned()),
                        locals: names,
                        borrow_stacks,
                    });
                    continue;
                }
                GlobalAlloc::TypeId { .. } => "TypeId".to_owned(),
            };
            entries.push(AllocInfo {
                alloc_id,
                ptr,
                base_addr,
                dealloc: false,
                kind: Some(MiriMemoryKind::Global.into()),
                size: Some(info.size.bytes_usize()),
                align: Some(info.align.bytes()),
                provenance_exposed,
                global: Some(global),
                locals: names,
                borrow_stacks,
            });
        } else if ecx.get_alloc_raw(alloc_id).discard_err().is_some() {
            let (kind, ..) = ptr_meta(alloc_id);
            entries.push(AllocInfo {
                alloc_id,
                ptr,
                base_addr,
                dealloc: false,
                kind,
                size: Some(info.size.bytes_usize()),
                align: Some(info.align.bytes()),
                provenance_exposed,
                global: None,
                locals: names,
                borrow_stacks,
            });
        }
    }

    entries.sort_unstable_by_key(|e| e.alloc_id);
    entries
}

/// Renders the source code of a MIR Body and highlights the source range
/// corresponding to a specific MIR Location using Ratatui styles.
fn capture_location(
    ecx: &MiriInterpCx<'_>,
    frame: &Frame<'_, Provenance, FrameExtra<'_>>,
) -> CurrentLocation {
    let sm = ecx.tcx.sess.source_map();
    let body = frame.body();
    let current_loc = match frame.current_loc() {
        Either::Left(loc) => loc,
        Either::Right(_) => todo!(),
    };

    // 1. Get the source range of the entire MIR Body
    let body_span = body.span;

    // 2. Get the Span corresponding to the current MIR Location (Statement or Terminator)
    let highlight_span =
        if current_loc.statement_index < body.basic_blocks[current_loc.block].statements.len() {
            body.basic_blocks[current_loc.block].statements[current_loc.statement_index]
                .source_info
                .span
        } else {
            body.basic_blocks[current_loc.block].terminator().source_info.span
        };

    // Use source_callsite() to resolve the span to the actual physical file
    // instead of inside a macro expansion if possible.
    let body_span_callsite = body_span.source_callsite();
    let highlight_span = highlight_span.source_callsite();

    let line_start = pos_to_line_nr(sm, highlight_span.lo());
    let line_end = pos_to_line_nr(sm, highlight_span.hi());

    // 3. Render source with highlight, falling back to reading via embeddable_name.
    let render_src = render_src(body_span_callsite, highlight_span, sm);

    let bb = body.basic_blocks.get(current_loc.block).unwrap();
    let (render_mir, render_mir_highlighted_idx) = current_mir(bb, current_loc.statement_index);

    CurrentLocation { render_src, line_start, line_end, render_mir, render_mir_highlighted_idx }
}

fn current_mir(bb: &BasicBlockData<'_>, stmt_idx: usize) -> (Vec<String>, u16) {
    let stmt_len = bb.statements.len();
    let hightlighted = if stmt_idx < stmt_len { stmt_idx } else { stmt_len };

    let mut v = Vec::with_capacity(stmt_len + 1);
    v.extend(bb.statements.iter().map(|stmt| format!("{:?}", stmt.kind)));
    v.push(format!("{:?}", bb.terminator().kind));
    (v, hightlighted.try_into().unwrap())
}
