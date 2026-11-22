//! Implements threads.

use std::cell::RefCell;
use std::mem;
use std::ops::Range;
use std::task::Poll;
use std::time::{Duration, SystemTime};

use rand::RngExt;
use rand::seq::IteratorRandom;
use rustc_abi::ExternAbi;
use rustc_data_structures::either::Either;
use rustc_data_structures::fx::FxHashMap;
use rustc_hir::def_id::DefId;
use rustc_index::{Idx, IndexVec};
use rustc_middle::mir::Mutability;
use rustc_middle::ty::Ty;
use rustc_middle::ty::layout::TyAndLayout;
use rustc_span::{DUMMY_SP, Span};
use rustc_target::spec::Os;

use crate::concurrency::GlobalDataRaceHandler;
use crate::concurrency::scheduler::SchedulingAction;
use crate::mirch::{
    self, config_stack_mem_size, kernel_code_paddr_to_vaddr, kernel_stack_end_addr, total_mem_size,
};
use crate::shims::tls;
use crate::*;

/// The argument type for the "unblock" callback, indicating why the thread got unblocked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UnblockKind {
    /// Operation completed successfully, thread continues normal execution.
    Ready,
    /// The operation did not complete within its specified duration.
    TimedOut,
}

/// Type alias for unblock callbacks, i.e. machine callbacks invoked when
/// a thread gets unblocked.
pub type DynUnblockCallback<'tcx> = DynMachineCallback<'tcx, UnblockKind>;

/// A thread identifier.
#[derive(Clone, Copy, Debug, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct ThreadId(u32);

impl ThreadId {
    pub fn to_u32(self) -> u32 {
        self.0
    }

    /// Create a new thread id from a `u32` without checking if this thread exists.
    pub fn new_unchecked(id: u32) -> Self {
        Self(id)
    }

    pub const MAIN_THREAD: ThreadId = ThreadId(0);
}

impl Idx for ThreadId {
    fn new(idx: usize) -> Self {
        ThreadId(u32::try_from(idx).unwrap())
    }

    fn index(self) -> usize {
        usize::try_from(self.0).unwrap()
    }
}

impl From<ThreadId> for u64 {
    fn from(t: ThreadId) -> Self {
        t.0.into()
    }
}

/// Keeps track of what the thread is blocked on.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum BlockReason {
    /// The thread tried to join the specified thread and is blocked until that
    /// thread terminates.
    Join(ThreadId),
    /// Waiting for time to pass.
    Sleep,
    /// Blocked on a mutex.
    Mutex,
    /// Blocked on a condition variable.
    Condvar,
    /// Blocked on a reader-writer lock.
    RwLock,
    /// Blocked on a semaphore.
    Semaphore,
    /// Blocked on a Futex variable.
    Futex,
    /// Blocked on an InitOnce.
    InitOnce,
    /// Blocked until a file description readiness is satisfied (e.g. epoll).
    Readiness,
    /// Blocked on eventfd.
    Eventfd,
    /// Blocked on virtual socket.
    VirtualSocket,
    /// Blocked on an IO operation.
    IO,
    /// Thread is paused indefinitely.
    Pause,
    /// Blocked for any reason related to GenMC, such as `assume` statements (GenMC mode only).
    /// Will be implicitly unblocked when GenMC schedules this thread again.
    Genmc,
}

/// The state of a thread.
enum ThreadState<'tcx> {
    /// The thread is enabled and can be executed.
    Enabled,
    /// The thread is blocked on something.
    Blocked { reason: BlockReason, deadline: Option<Deadline>, callback: DynUnblockCallback<'tcx> },
    /// The thread has terminated its execution. We do not delete terminated
    /// threads (FIXME: why?).
    Terminated,
}

impl<'tcx> std::fmt::Debug for ThreadState<'tcx> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Enabled => write!(f, "Enabled"),
            Self::Blocked { reason, deadline, .. } =>
                f.debug_struct("Blocked")
                    .field("reason", reason)
                    .field("deadline", deadline)
                    .finish(),
            Self::Terminated => write!(f, "Terminated"),
        }
    }
}

impl<'tcx> ThreadState<'tcx> {
    fn is_enabled(&self) -> bool {
        matches!(self, ThreadState::Enabled)
    }

    fn is_terminated(&self) -> bool {
        matches!(self, ThreadState::Terminated)
    }

    fn is_blocked_on(&self, reason: BlockReason) -> bool {
        matches!(self, ThreadState::Blocked { reason: actual_reason, .. } if *actual_reason == reason)
    }
}

/// The join status of a thread.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum ThreadJoinStatus {
    /// The thread can be joined.
    Joinable,
    /// A thread is detached if its join handle was destroyed and no other
    /// thread can join it.
    Detached,
    /// The thread was already joined by some thread and cannot be joined again.
    Joined,
}

#[derive(Clone)]
pub(crate) struct StackAddrRecord<'tcx> {
    pub addr: u64,
    pub ret_ty: Ty<'tcx>,
    pub ret_ty_size: u64,
    pub ret_ty_align: u64,
}

/// A thread.
pub struct Thread<'tcx> {
    state: ThreadState<'tcx>,

    /// Name of the thread.
    thread_name: Option<Vec<u8>>,

    /// The virtual call stack.
    stack: Vec<Frame<'tcx, Provenance, FrameExtra<'tcx>>>,

    /// Records for the addresses of the stack frames in the current thread.
    pub(crate) stack_addr_records: Vec<StackAddrRecord<'tcx>>,

    /// The stack address for the next stack variable.
    pub(crate) next_stack_vaddr: RefCell<u64>,

    /// The stack bottom of the current thread.
    pub(crate) stack_bottom: u64,

    /// A span that explains where the thread (or more specifically, its current root
    /// frame) "comes from".
    pub(crate) origin_span: Span,

    /// The function to call when the stack ran empty, to figure out what to do next.
    /// Conceptually, this is the interpreter implementation of the things that happen 'after' the
    /// Rust language entry point for this thread returns (usually implemented by the C or OS runtime).
    /// (`None` is an error, it means the callback has not been set up yet or is actively running.)
    pub(crate) on_stack_empty: Option<StackEmptyCallback<'tcx>>,

    /// The index of the topmost user-relevant frame in `stack`. This field must contain
    /// the value produced by `get_top_user_relevant_frame`.
    /// This field is a cache to reduce how often we call that method. The cache is manually
    /// maintained inside `MiriMachine::after_stack_push` and `MiriMachine::after_stack_pop`.
    top_user_relevant_frame: Option<usize>,

    /// The join status.
    join_status: ThreadJoinStatus,

    /// Stack of active unwind payloads for the current thread. Used for storing
    /// the argument of the call to `miri_start_unwind` (the payload) when unwinding.
    /// This is pointer-sized, and matches the `Payload` type in `src/libpanic_unwind/miri.rs`.
    ///
    /// In real unwinding, the payload gets passed as an argument to the landing pad,
    /// which then forwards it to 'Resume'. However this argument is implicit in MIR,
    /// so we have to store it out-of-band. When there are multiple active unwinds,
    /// the innermost one is always caught first, so we can store them as a stack.
    pub(crate) unwind_payloads: Vec<ImmTy<'tcx>>,

    /// Last OS error location in memory. It is a 32-bit integer.
    pub(crate) last_error: Option<MPlaceTy<'tcx>>,
}

pub type StackEmptyCallback<'tcx> =
    Box<dyn FnMut(&mut MiriInterpCx<'tcx>) -> InterpResult<'tcx, Poll<()>> + 'tcx>;

impl<'tcx> Thread<'tcx> {
    /// Get the name of the current thread if it was set.
    fn thread_name(&self) -> Option<&[u8]> {
        self.thread_name.as_deref()
    }

    /// Return whether this thread is enabled or not.
    pub fn is_enabled(&self) -> bool {
        self.state.is_enabled()
    }

    pub fn is_terminated(&self) -> bool {
        self.state.is_terminated()
    }

    pub fn is_blocked_on(&self, reason: BlockReason) -> bool {
        self.state.is_blocked_on(reason)
    }

    /// Get the name of the current thread for display purposes; will include thread ID if not set.
    pub fn thread_display_name(&self, id: ThreadId) -> String {
        if let Some(ref thread_name) = self.thread_name {
            String::from_utf8_lossy(thread_name).into_owned()
        } else {
            format!("unnamed-{}", id.index())
        }
    }

    /// Return the top user-relevant frame, if there is one. `skip` indicates how many top frames
    /// should be skipped.
    /// Note that the choice to return `None` here when there is no user-relevant frame is part of
    /// justifying the optimization that only pushes of user-relevant frames require updating the
    /// `top_user_relevant_frame` field.
    fn compute_top_user_relevant_frame(&self, skip: usize) -> Option<usize> {
        // We are search for the frame with maximum relevance.
        let mut best = None;
        for (idx, frame) in self.stack.iter().enumerate().rev().skip(skip) {
            let relevance = frame.extra.user_relevance;
            if relevance == u8::MAX {
                // We can short-circuit this search.
                return Some(idx);
            }
            if best.is_none_or(|(_best_idx, best_relevance)| best_relevance < relevance) {
                // The previous best frame has strictly worse relevance, so despite us being lower
                // in the stack, we win.
                best = Some((idx, relevance));
            }
        }
        best.map(|(idx, _relevance)| idx)
    }

    /// Re-compute the top user-relevant frame from scratch. `skip` indicates how many top frames
    /// should be skipped.
    pub fn recompute_top_user_relevant_frame(&mut self, skip: usize) {
        self.top_user_relevant_frame = self.compute_top_user_relevant_frame(skip);
    }

    /// Set the top user-relevant frame to the given value. Must be equal to what
    /// `get_top_user_relevant_frame` would return!
    pub fn set_top_user_relevant_frame(&mut self, frame_idx: usize) {
        debug_assert_eq!(Some(frame_idx), self.compute_top_user_relevant_frame(0));
        self.top_user_relevant_frame = Some(frame_idx);
    }

    /// Returns the topmost frame that is considered user-relevant, or the
    /// top of the stack if there is no such frame, or `None` if the stack is empty.
    pub fn top_user_relevant_frame(&self) -> Option<usize> {
        // This can be called upon creation of an allocation. We create allocations while setting up
        // parts of the Rust runtime when we do not have any stack frames yet, so we need to handle
        // empty stacks.
        self.top_user_relevant_frame.or_else(|| self.stack.len().checked_sub(1))
    }

    pub fn current_user_relevance(&self) -> u8 {
        self.top_user_relevant_frame()
            .map(|frame_idx| self.stack[frame_idx].extra.user_relevance)
            .unwrap_or(0)
    }

    pub fn current_user_relevant_span(&self) -> Span {
        debug_assert_eq!(self.top_user_relevant_frame, self.compute_top_user_relevant_frame(0));
        self.top_user_relevant_frame()
            .map(|frame_idx| self.stack[frame_idx].current_span())
            .unwrap_or(rustc_span::DUMMY_SP)
    }

    pub fn last_frame(&self) -> Option<&Frame<'tcx, Provenance, FrameExtra<'tcx>>> {
        self.stack.last()
    }

    pub fn top_user_frame(&self) -> Option<&Frame<'tcx, Provenance, FrameExtra<'tcx>>> {
        self.top_user_relevant_frame().map(|frame_idx| &self.stack[frame_idx])
    }

    pub fn display_stack_records(&self) -> String {
        self.stack_addr_records
            .iter()
            .map(|addr| format!("  {:#x}", addr.addr))
            .collect::<Vec<String>>()
            .join(",\n")
    }
}

impl<'tcx> std::fmt::Debug for Thread<'tcx> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}({:?}, {:?})",
            String::from_utf8_lossy(self.thread_name().unwrap_or(b"<unnamed>")),
            self.state,
            self.join_status
        )
    }
}

impl<'tcx> Thread<'tcx> {
    fn new(
        name: Option<&str>,
        on_stack_empty: Option<StackEmptyCallback<'tcx>>,
        stack_range: Range<u64>,
    ) -> Self {
        log!("[Thread::new] stack_range={:#x}..{:#x}", stack_range.start, stack_range.end);
        Self {
            state: ThreadState::Enabled,
            thread_name: name.map(|name| Vec::from(name.as_bytes())),
            stack: Vec::new(),
            stack_addr_records: Vec::new(),
            next_stack_vaddr: RefCell::new(stack_range.end),
            stack_bottom: stack_range.start,
            origin_span: DUMMY_SP,
            top_user_relevant_frame: None,
            join_status: ThreadJoinStatus::Joinable,
            unwind_payloads: Vec::new(),
            last_error: None,
            on_stack_empty,
        }
    }
}

impl VisitProvenance for Thread<'_> {
    fn visit_provenance(&self, visit: &mut VisitWith<'_>) {
        let Thread {
            unwind_payloads: panic_payload,
            last_error,
            stack,
            stack_addr_records: _,
            next_stack_vaddr: _,
            stack_bottom: _,
            origin_span: _,
            top_user_relevant_frame: _,
            state: _,
            thread_name: _,
            join_status: _,
            on_stack_empty: _, // we assume the closure captures no GC-relevant state
        } = self;

        for payload in panic_payload {
            payload.visit_provenance(visit);
        }
        last_error.visit_provenance(visit);
        for frame in stack {
            frame.visit_provenance(visit)
        }
    }
}

impl VisitProvenance for Frame<'_, Provenance, FrameExtra<'_>> {
    fn visit_provenance(&self, visit: &mut VisitWith<'_>) {
        let return_place = self.return_place();
        let locals = self.locals();
        let Frame {
            extra,
            // There are some private fields we cannot access; they contain no tags.
            ..
        } = self;

        // Return place.
        return_place.visit_provenance(visit);
        // Locals.
        for local in locals.iter() {
            // We only need the provenance so it's good for this to not be a real read.
            match local.as_mplace_or_imm_ghost() {
                None => {}
                Some(Either::Left((ptr, meta))) => {
                    ptr.visit_provenance(visit);
                    meta.visit_provenance(visit);
                }
                Some(Either::Right(imm)) => {
                    imm.visit_provenance(visit);
                }
            }
        }

        extra.visit_provenance(visit);
    }
}

/// An error signaling that the requested thread doesn't exist or has terminated.
#[derive(Debug, Copy, Clone)]
pub enum ThreadLookupError {
    /// No thread with this ID exists.
    InvalidId,
    /// The thread exists but has already terminated.
    Terminated(ThreadId),
}

/// Represents the state of a thread stack in the memory pool.
#[derive(Debug, Clone)]
struct StackState {
    base_vaddr: u64,
    size: u64,
    /// Only true if the stack is being used for a thread.
    using: bool,
}

impl StackState {
    fn new(base_vaddr: u64, size: u64) -> Self {
        Self { base_vaddr, size, using: false }
    }

    fn range(&self) -> Range<u64> {
        self.base_vaddr..(self.base_vaddr + self.size)
    }

    fn mark_using(&mut self) {
        self.using = true;
    }

    fn clear_using(&mut self) {
        self.using = false;
    }
}

/// A set of threads.
#[derive(Debug)]
pub struct ThreadManager<'tcx> {
    /// Identifier of the currently active thread.
    active_thread: ThreadId,
    /// Identifier of the currently active CPU number.
    pub active_cpu: usize,
    /// The CPU number that will be active in the next iteration.
    ///
    /// The machine will keep the old executed CPU if the value is `None`.
    next_cpu: Option<usize>,
    /// An array stores the mapping between CPU number and the `ThreadId`.
    cpu_to_threads: Vec<Option<ThreadId>>,
    /// An array stores the base virtual address of cpu local variables for each CPU.
    pub cpu_local_base: Vec<usize>,
    /// Threads used in the program.
    ///
    /// Note that this vector also contains terminated threads.
    threads: IndexVec<ThreadId, Thread<'tcx>>,
    /// A mapping from a thread-local static to the thread specific allocation.
    thread_local_allocs: FxHashMap<(DefId, ThreadId), StrictPointer>,
    /// A flag that indicates that we should change the active thread.
    /// Completely ignored in GenMC mode.
    pub(super) yield_active_thread: bool,
    /// A flag that indicates that we should do round robin scheduling of threads else randomized scheduling is used.
    fixed_scheduling: bool,
    /// An array records ID of the next thread to execute for each CPU.
    next_thread: Vec<Option<ThreadId>>,
    /// Reusable stack allocation regions.
    stack_pool: StackPool,
}

#[derive(Debug)]
struct StackPool {
    /// Stack state pool tracking all allocated stacks (Best-Fit strategy).
    stack_pool: Vec<StackState>,
    /// Next unique ID for allocating new stacks when pool is empty.
    next_stack_id: usize,
}

impl VisitProvenance for ThreadManager<'_> {
    fn visit_provenance(&self, visit: &mut VisitWith<'_>) {
        let ThreadManager { threads, thread_local_allocs, .. } = self;

        for thread in threads {
            thread.visit_provenance(visit);
        }
        for ptr in thread_local_allocs.values() {
            ptr.visit_provenance(visit);
        }
    }
}

impl<'tcx> ThreadManager<'tcx> {
    /// Reusable stack memory regions for stack allocations.
    fn stack_pool(&mut self) -> &mut Vec<StackState> {
        &mut self.stack_pool.stack_pool
    }

    /// Get a stack range for a new thread.
    /// Uses Best-Fit strategy to find the smallest suitable freed stack that fits the required size.
    fn get_stack_range(&mut self, stack_mem_size: u64) -> Range<u64> {
        // Find the best-fit entry from stack pool (smallest freed stack that fits)
        let best_fit = self
            .stack_pool()
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, s)| !s.using && s.size >= stack_mem_size)
            .min_by_key(|(_, s)| s.size);

        if let Some((idx, _)) = best_fit {
            let stack = &mut self.stack_pool()[idx];
            assert!(!stack.using, "The stack {stack:?} must not be using!");
            stack.mark_using();
            return stack.range();
        }

        // Allocate a new stack when no suitable fit is found
        let id = self.stack_pool.next_stack_id;
        self.stack_pool.next_stack_id += 1;

        let step = stack_mem_size as usize;
        let base = kernel_stack_end_addr();
        let start = base + id * step;
        let end = base + (id + 1) * step;
        assert!(end < total_mem_size());

        let vaddr_base = kernel_code_paddr_to_vaddr(start) as u64;
        let vaddr_end = kernel_code_paddr_to_vaddr(end) as u64;
        let range = vaddr_base..vaddr_end;

        // Add to stack pool with using=true
        self.stack_pool().push({
            let mut stack = StackState::new(vaddr_base, stack_mem_size);
            stack.mark_using();
            stack
        });

        range
    }

    pub(crate) fn new(config: &MiriConfig) -> Self {
        let mut threads = IndexVec::new();
        // Create the main thread and add it to the list of threads.
        threads.push(Thread::new(
            Some("main"),
            None,
            kernel_code_paddr_to_vaddr(mirch::kernel_stack_start_addr()) as u64
                ..kernel_code_paddr_to_vaddr(mirch::kernel_stack_end_addr()) as u64,
        ));
        let num_cpus = config.kmiri_toml.as_ref().map(|toml| toml.smp().num_cpus()).unwrap_or(1);
        // Only CPU 0 runs at reset; the others start with `kern_miri_start_cpu`.
        let mut cpu_to_threads = vec![None; num_cpus];
        cpu_to_threads[0] = Some(ThreadId::MAIN_THREAD);
        // The CPU-local base register holds its reset value 0 until the kernel's boot
        // code sets it (`kern_miri_set_cpu_local_base`), as on hardware.
        let cpu_local_base = vec![0; num_cpus];
        Self {
            active_thread: ThreadId::MAIN_THREAD,
            cpu_to_threads,
            cpu_local_base,
            active_cpu: 0,
            next_cpu: None,
            threads,
            thread_local_allocs: Default::default(),
            yield_active_thread: false,
            fixed_scheduling: config.fixed_scheduling,
            next_thread: vec![None; num_cpus],
            stack_pool: StackPool { stack_pool: Vec::new(), next_stack_id: 1 },
        }
    }

    pub(crate) fn init(
        ecx: &mut MiriInterpCx<'tcx>,
        on_main_stack_empty: StackEmptyCallback<'tcx>,
    ) {
        ecx.machine.threads.threads[ThreadId::MAIN_THREAD].on_stack_empty =
            Some(on_main_stack_empty);
        if ecx.tcx.sess.target.os != Os::Windows {
            // The main thread can *not* be joined on except on windows.
            ecx.machine.threads.threads[ThreadId::MAIN_THREAD].join_status =
                ThreadJoinStatus::Detached;
        }
    }

    /// Returns the `ThreadId` for the given raw thread id.
    /// Returns `Err(ThreadNotFound::InvalidId)` if the id is out of range, or
    /// `Err(ThreadNotFound::Terminated(id))` if the thread exists but has terminated.
    pub fn thread_id_try_from(&self, id: impl TryInto<u32>) -> Result<ThreadId, ThreadLookupError> {
        if let Ok(id) = id.try_into()
            && usize::try_from(id).is_ok_and(|id| id < self.threads.len())
        {
            let thread_id = ThreadId(id);
            if self.threads[thread_id].state.is_terminated() {
                Err(ThreadLookupError::Terminated(thread_id))
            } else {
                Ok(thread_id)
            }
        } else {
            Err(ThreadLookupError::InvalidId)
        }
    }

    pub fn current_cpu_local_base(&self) -> usize {
        self.cpu_local_base[self.active_cpu]
    }

    /// The number of simulated CPUs, including those not started yet.
    pub fn num_cpus(&self) -> usize {
        self.cpu_to_threads.len()
    }

    pub fn is_cpu_started(&self, cpu: usize) -> bool {
        self.cpu_to_threads[cpu].is_some()
    }

    /// Whether a CPU other than CPU 0 is still running a context.
    pub fn other_cpu_running(&self) -> bool {
        self.cpu_to_threads[1..].iter().flatten().any(|&t| !self.threads[t].state.is_terminated())
    }

    /// Makes `thread` the context that CPU `cpu` runs from now on.
    pub fn start_cpu(&mut self, cpu: usize, thread: ThreadId) {
        assert!(self.cpu_to_threads[cpu].is_none());
        self.cpu_to_threads[cpu] = Some(thread);
    }

    /// The started CPUs, other than the active one, whose current context can take a step.
    pub fn other_runnable_cpus(&self) -> Vec<usize> {
        (0..self.num_cpus())
            .filter(|&cpu| {
                cpu != self.active_cpu
                    && self.cpu_to_threads[cpu].is_some_and(|t| self.threads[t].state.is_enabled())
            })
            .collect()
    }

    /// Moves execution to the CPU chosen by a CPU switch, if one was requested.
    /// The thread that CPU was running continues where it stopped.
    pub fn apply_cpu_switch(&mut self) {
        if let Some(cpu) = self.next_cpu.take() {
            self.active_cpu = cpu;
            self.active_thread = self.cpu_to_threads[cpu].unwrap();
        }
    }

    /// Check if we have an allocation for the given thread local static for the
    /// active thread.
    fn get_thread_local_alloc_id(&self, def_id: DefId) -> Option<StrictPointer> {
        self.thread_local_allocs.get(&(def_id, self.active_thread)).cloned()
    }

    /// Set the pointer for the allocation of the given thread local
    /// static for the active thread.
    ///
    /// Panics if a thread local is initialized twice for the same thread.
    fn set_thread_local_alloc(&mut self, def_id: DefId, ptr: StrictPointer) {
        self.thread_local_allocs.try_insert((def_id, self.active_thread), ptr).unwrap();
    }

    /// Borrow the stack of the active thread.
    pub fn active_thread_stack(&self) -> &[Frame<'tcx, Provenance, FrameExtra<'tcx>>] {
        &self.threads[self.active_thread].stack
    }

    /// Mutably borrow the stack of the active thread.
    pub fn active_thread_stack_mut(
        &mut self,
    ) -> &mut Vec<Frame<'tcx, Provenance, FrameExtra<'tcx>>> {
        &mut self.threads[self.active_thread].stack
    }

    pub(super) fn all_threads(&self) -> impl Iterator<Item = (ThreadId, &Thread<'tcx>)> {
        self.threads.iter_enumerated()
    }

    pub fn all_blocked_stacks(
        &self,
    ) -> impl Iterator<Item = (ThreadId, &[Frame<'tcx, Provenance, FrameExtra<'tcx>>])> {
        self.threads
            .iter_enumerated()
            .filter(|(_id, t)| matches!(t.state, ThreadState::Blocked { .. }))
            .map(|(id, t)| (id, &t.stack[..]))
    }

    /// Create a new thread and returns its id.
    ///
    /// The new thread will use the `stack_range` as its stack.
    fn create_thread(
        &mut self,
        on_stack_empty: StackEmptyCallback<'tcx>,
        stack: Stack,
    ) -> ThreadId {
        let stack_range = match stack {
            Stack::Range { start, end } => start as u64..end as u64,
            Stack::Size(size) => self.get_stack_range(size as u64),
        };
        self.threads.push(Thread::new(None, Some(on_stack_empty), stack_range))
    }

    /// Set an active thread and return the id of the thread that was active before.
    pub(super) fn set_active_thread(&mut self, id: ThreadId) -> ThreadId {
        assert!(id.index() < self.threads.len());
        info!(
            "---------- Now executing on thread `{}` (previous: `{}`) ----------------------------------------",
            self.get_thread_display_name(id),
            self.get_thread_display_name(self.active_thread)
        );
        std::mem::replace(&mut self.active_thread, id)
    }

    /// Get the id of the currently active thread.
    pub fn active_thread(&self) -> ThreadId {
        self.active_thread
    }

    /// Get the total number of threads that were ever spawn by this program.
    pub fn get_total_thread_count(&self) -> usize {
        self.threads.len()
    }

    /// Get the total of threads that are currently live, i.e., not yet terminated.
    /// (They might be blocked.)
    pub fn get_live_thread_count(&self) -> usize {
        self.threads.iter().filter(|t| !t.state.is_terminated()).count()
    }

    /// Has the given thread terminated?
    fn has_terminated(&self, thread_id: ThreadId) -> bool {
        self.threads[thread_id].state.is_terminated()
    }

    /// Have all threads terminated?
    fn have_all_terminated(&self) -> bool {
        self.threads.iter().all(|thread| thread.state.is_terminated())
    }

    /// Enable the thread for execution. The thread must be terminated.
    fn enable_thread(&mut self, thread_id: ThreadId) {
        assert!(self.has_terminated(thread_id));
        self.threads[thread_id].state = ThreadState::Enabled;
    }

    /// Get a mutable borrow of the currently active thread.
    pub fn active_thread_mut(&mut self) -> &mut Thread<'tcx> {
        &mut self.threads[self.active_thread]
    }

    /// Get a shared borrow of the currently active thread.
    pub fn active_thread_ref(&self) -> &Thread<'tcx> {
        &self.threads[self.active_thread]
    }

    pub fn thread_ref(&self, thread_id: ThreadId) -> &Thread<'tcx> {
        &self.threads[thread_id]
    }

    /// Mark the thread as detached, which means that no other thread will try
    /// to join it and the thread is responsible for cleaning up.
    ///
    /// `allow_terminated_joined` allows detaching joined threads that have already terminated.
    /// This matches Windows's behavior for `CloseHandle`.
    ///
    /// See <https://docs.microsoft.com/en-us/windows/win32/procthread/thread-handles-and-identifiers>:
    /// > The handle is valid until closed, even after the thread it represents has been terminated.
    fn detach_thread(&mut self, id: ThreadId, allow_terminated_joined: bool) -> InterpResult<'tcx> {
        // NOTE: In GenMC mode, we treat detached threads like regular threads that are never joined, so there is no special handling required here.
        trace!("detaching {:?}", id);

        let is_ub = if allow_terminated_joined && self.threads[id].state.is_terminated() {
            // "Detached" in particular means "not yet joined". Redundant detaching is still UB.
            self.threads[id].join_status == ThreadJoinStatus::Detached
        } else {
            self.threads[id].join_status != ThreadJoinStatus::Joinable
        };
        if is_ub {
            throw_ub_format!("trying to detach thread that was already detached or joined");
        }

        self.threads[id].join_status = ThreadJoinStatus::Detached;
        interp_ok(())
    }

    /// Set the name of the given thread.
    pub fn set_thread_name(&mut self, thread: ThreadId, new_thread_name: Vec<u8>) {
        self.threads[thread].thread_name = Some(new_thread_name);
    }

    /// Get the name of the given thread.
    pub fn get_thread_name(&self, thread: ThreadId) -> Option<&[u8]> {
        self.threads[thread].thread_name()
    }

    pub fn get_thread_display_name(&self, thread: ThreadId) -> String {
        self.threads[thread].thread_display_name(thread)
    }

    /// Put the thread into the blocked state.
    fn block_thread(
        &mut self,
        reason: BlockReason,
        deadline: Option<Deadline>,
        callback: DynUnblockCallback<'tcx>,
    ) {
        let state = &mut self.threads[self.active_thread].state;
        assert!(state.is_enabled());
        *state = ThreadState::Blocked { reason, deadline, callback }
    }

    pub fn fixed_scheduling(&self) -> bool {
        self.fixed_scheduling
    }

    /// Switches to the thread with thread ID `thread_id`.
    pub fn switch_to(&mut self, thread_id: ThreadId) {
        self.next_thread[self.active_cpu] = Some(thread_id);
    }

    pub fn schedule_switch_thread_and_cpu(
        &mut self,
    ) -> Option<InterpResult<'tcx, SchedulingAction>> {
        // A terminating kernel task may have queued a successor from on_stack_empty.
        // Honor it before falling back to the main thread.
        if let Some(id) = self.next_thread[self.active_cpu] {
            assert_ne!(self.active_thread, id);
            if let Some(other) = self.cpu_to_threads.iter().position(|&t| t == Some(id)) {
                return Some(
                    Err(rustc_middle::mir::interpret::InterpErrorKind::MachineStop(Box::new(
                        TerminationInfo::Abort(format!(
                            "the program switched CPU {} to a context that CPU {other} is running",
                            self.active_cpu
                        )),
                    )))
                    .into(),
                );
            }
            let old_id = self.active_thread;
            self.active_thread = id;
            self.cpu_to_threads[self.active_cpu] = Some(id);
            self.next_thread[self.active_cpu] = None;
            if self.threads[self.active_thread].state.is_enabled() {
                info!(
                    "---------- Now executing on thread `{}` (previous: `{}`) cpu: {:?}----------------------------------------",
                    self.get_thread_display_name(id),
                    self.get_thread_display_name(old_id),
                    self.active_cpu,
                );
                if let Some(next_cpu) = self.next_cpu {
                    self.active_cpu = next_cpu;
                    self.active_thread = self.cpu_to_threads[next_cpu].unwrap();
                    self.next_cpu = None;
                    return Some(interp_ok(SchedulingAction::ExecuteStep));
                }
                return Some(interp_ok(SchedulingAction::ExecuteStep));
            } else {
                return Some(
                    Err(rustc_middle::mir::interpret::InterpErrorKind::MachineStop(Box::new(
                        TerminationInfo::GlobalDeadlock,
                    )))
                    .into(),
                );
            }
        }
        if let ThreadState::Terminated = self.threads[self.active_thread].state {
            if self.active_cpu != 0 {
                // The context a started CPU was running returned with no successor:
                // that CPU stops, and execution continues on another running CPU.
                self.cpu_to_threads[self.active_cpu] = None;
                let cpu = self.other_runnable_cpus().first().copied().unwrap_or(0);
                self.active_cpu = cpu;
                self.active_thread = self.cpu_to_threads[cpu].unwrap_or(ThreadId::MAIN_THREAD);
                return Some(interp_ok(SchedulingAction::ExecuteStep));
            }
            // CPU 0 goes back to the bootstrap context.
            self.active_thread = ThreadId::MAIN_THREAD;
            self.cpu_to_threads[0] = Some(ThreadId::MAIN_THREAD);
            return Some(interp_ok(SchedulingAction::ExecuteStep));
        }
        if self.next_cpu.is_some() {
            self.apply_cpu_switch();
            return Some(interp_ok(SchedulingAction::ExecuteStep));
        }
        None
    }
}

pub enum Stack {
    Range { start: usize, end: usize },
    Size(usize),
}

// Public interface to thread management.
impl<'tcx> EvalContextExt<'tcx> for crate::MiriInterpCx<'tcx> {}
pub trait EvalContextExt<'tcx>: crate::MiriInterpCxExt<'tcx> {
    #[inline]
    fn thread_id_try_from(&self, id: impl TryInto<u32>) -> Result<ThreadId, ThreadLookupError> {
        self.eval_context_ref().machine.threads.thread_id_try_from(id)
    }

    /// Get a thread-specific allocation id for the given thread-local static.
    /// If needed, allocate a new one.
    fn get_or_create_thread_local_alloc(
        &mut self,
        def_id: DefId,
    ) -> InterpResult<'tcx, StrictPointer> {
        let ecx = self.eval_context_mut();
        let tcx = ecx.tcx;
        if let Some(old_alloc) = ecx.machine.threads.get_thread_local_alloc_id(def_id) {
            // We already have a thread-specific allocation id for this
            // thread-local static.
            interp_ok(old_alloc)
        } else {
            // We need to allocate a thread-specific allocation id for this
            // thread-local static.
            // First, we compute the initial value for this static.
            if tcx.is_foreign_item(def_id) {
                throw_unsup_format!("foreign thread-local statics are not supported");
            }
            let params = ecx.machine.get_default_alloc_params();
            let alloc = ecx.ctfe_query(|tcx| tcx.eval_static_initializer(def_id))?;
            // We make a full copy of this allocation.
            let mut alloc = alloc.inner().adjust_from_tcx(
                &ecx.tcx,
                |bytes, align| {
                    interp_ok(MiriAllocBytes::from_bytes(
                        std::borrow::Cow::Borrowed(bytes),
                        align,
                        params,
                    ))
                },
                |ptr| ecx.global_root_pointer(ptr),
            )?;
            // This allocation will be deallocated when the thread dies, so it is not in read-only memory.
            alloc.mutability = Mutability::Mut;
            // Create a fresh allocation with this content.
            let ptr = ecx.insert_allocation(alloc, MiriMemoryKind::Tls.into())?;
            ecx.machine.threads.set_thread_local_alloc(def_id, ptr);
            interp_ok(ptr)
        }
    }

    /// Start a regular (non-main) thread.
    #[inline]
    fn start_regular_thread(
        &mut self,
        thread: Option<MPlaceTy<'tcx>>,
        start_routine: Pointer,
        start_abi: ExternAbi,
        func_arg: ImmTy<'tcx>,
        ret_layout: TyAndLayout<'tcx>,
        stack_range: Option<Stack>,
    ) -> InterpResult<'tcx, ThreadId> {
        let ecx = self.eval_context_ref();
        let stack_range = stack_range.unwrap_or_else(|| {
            Stack::Size(
                ecx.machine
                    .kmiri_toml
                    .as_ref()
                    .map(|t| t.stack_mem_size())
                    .unwrap_or_else(config_stack_mem_size) as usize,
            )
        });

        let ecx = self.eval_context_mut();

        // Create the new thread with automatic stack allocation/reuse
        let current_span = ecx.machine.debugger_current_span();
        let new_thread_id = ecx.machine.threads.create_thread(
            {
                let mut state = tls::TlsDtorsState::new_for_thread_finish();
                Box::new(move |m| state.on_stack_empty(m))
            },
            stack_range,
        );
        match &mut ecx.machine.data_race {
            GlobalDataRaceHandler::None => {}
            GlobalDataRaceHandler::Vclocks(data_race) =>
                data_race.thread_created(&ecx.machine.threads, new_thread_id, current_span),
            GlobalDataRaceHandler::Genmc(genmc_ctx) =>
                genmc_ctx.handle_thread_create(
                    &ecx.machine.threads,
                    start_routine,
                    &func_arg,
                    new_thread_id,
                )?,
        }
        // Write the current thread-id, switch to the next thread later
        // to treat this write operation as occurring on the current thread.
        if let Some(thread_info_place) = thread {
            ecx.write_scalar(
                Scalar::from_uint(new_thread_id.to_u32(), thread_info_place.layout.size),
                &thread_info_place,
            )?;
        }

        // Finally switch to new thread so that we can push the first stackframe.
        // After this all accesses will be treated as occurring in the new thread.
        let old_thread_id = ecx.machine.threads.set_active_thread(new_thread_id);

        // The child inherits its parent's cpu affinity.
        // Skips this if `machine.thread_cpu_affinity` is not initialized.
        if let Some(thread_cpu_affinity) = &mut ecx.machine.thread_cpu_affinity
            && let Some(cpuset) = thread_cpu_affinity.get(&old_thread_id).cloned()
        {
            thread_cpu_affinity.insert(new_thread_id, cpuset);
        }

        // Perform the function pointer load in the new thread frame.
        let instance = ecx.get_ptr_fn(start_routine)?.as_instance()?;

        // Note: the returned value is currently ignored (see the FIXME in
        // pthread_join in shims/unix/thread.rs) because the Rust standard library does not use
        // it.
        let ret_place = ecx.allocate(ret_layout, MiriMemoryKind::Machine.into())?;

        ecx.call_thread_root_function(
            instance,
            start_abi,
            &[func_arg],
            Some(&ret_place),
            current_span,
        )?;

        // Restore the old active thread frame.
        ecx.machine.threads.set_active_thread(old_thread_id);

        interp_ok(new_thread_id)
    }

    /// Replace the active thread with an empty one, and a new on-empty handler.
    /// The memory of the old stack is leaked.
    /// Used to implement `exit`.
    fn replace_active_thread_stack(&mut self, on_stack_empty: StackEmptyCallback<'tcx>) {
        let ecx = self.eval_context_mut();
        let thread = ecx.active_thread_mut();
        let span = thread.current_user_relevant_span();

        thread.stack.clear();
        thread.origin_span = span;
        thread.top_user_relevant_frame = None;
        thread.on_stack_empty = Some(on_stack_empty);
    }

    /// Handles thread termination of the active thread: wakes up threads joining on this one,
    /// and frees the thread's thread-local statics. `for_exit` indicates whether we are
    /// about to exit the program; that changes some details.
    ///
    /// This is called by the eval loop when a thread's on_stack_empty returns `Ready`.
    fn terminate_active_thread(&mut self, for_exit: bool) -> InterpResult<'tcx> {
        let ecx = self.eval_context_mut();

        // Mark thread as terminated.
        let thread_id = ecx.active_thread();
        let thread = ecx.active_thread_mut();
        assert!(
            for_exit || thread.stack.is_empty(),
            "only threads with an empty stack can be terminated"
        );

        // Save stack bottom for recycling (we use it to find the corresponding StackState)
        let stack_bottom = thread.stack_bottom;

        thread.state = ThreadState::Terminated;

        // Deallocate TLS.
        {
            let mut free_tls_statics = Vec::new();
            ecx.machine.threads.thread_local_allocs.retain(|&(_def_id, tid), &mut ptr| {
                if tid != thread_id {
                    // A different thread, keep this static around.
                    return true;
                }
                // Delete this static from the map and from memory.
                // We cannot free directly here as we cannot use `?` in this context.
                free_tls_statics.push(ptr);
                false
            });
            // Now free the TLS statics.
            for ptr in free_tls_statics {
                if for_exit {
                    // Leak the thread-local statics rather than freeing them.
                    if let Some(alloc) = ptr.provenance.get_alloc_id() {
                        trace!("Thread-local static leaked and stored as static root: {:?}", alloc);
                        ecx.machine.static_roots.push(alloc);
                    }
                } else {
                    ecx.deallocate_ptr(ptr.into(), None, MiriMemoryKind::Tls.into())?;
                }
            }
        }

        match &mut ecx.machine.data_race {
            GlobalDataRaceHandler::None => {}
            GlobalDataRaceHandler::Vclocks(data_race) =>
                data_race.thread_terminated(&ecx.machine.threads),
            GlobalDataRaceHandler::Genmc(genmc_ctx) => {
                // Inform GenMC that the thread finished.
                // This needs to happen once all accesses to the thread are done, including freeing any TLS statics.
                genmc_ctx.handle_thread_finish(&ecx.machine.threads)
            }
        }

        // Unblock joining threads.
        let unblock_reason = BlockReason::Join(thread_id);
        let threads = &ecx.machine.threads.threads;
        let joining_threads = threads
            .iter_enumerated()
            .filter(|(_, thread)| thread.state.is_blocked_on(unblock_reason))
            .map(|(id, _)| id)
            .collect::<Vec<_>>();
        for thread in joining_threads {
            ecx.unblock_thread(thread, unblock_reason)?;
        }

        // Kernel tasks return to their queued successor, or to the bootstrap
        // thread. That handoff waits for this entry and its destructors to end.
        // Ordinary std/pthread threads must still use their explicit join path.
        let cpu = ecx.machine.threads.active_cpu;
        let successor =
            ecx.machine.threads.next_thread[cpu].or((cpu == 0).then_some(ThreadId::MAIN_THREAD));
        // On a started CPU, a context that returns with no successor just stops that
        // CPU: nothing runs after it there, so it synchronizes with nobody.
        if ecx.machine.kernel_threads.contains(&thread_id)
            && let Some(next) = successor
        {
            let old = ecx.machine.threads.set_active_thread(next);
            match &mut ecx.machine.data_race {
                GlobalDataRaceHandler::None => {}
                GlobalDataRaceHandler::Vclocks(data_race) =>
                    data_race.thread_joined(&ecx.machine.threads, thread_id),
                GlobalDataRaceHandler::Genmc(genmc_ctx) =>
                    genmc_ctx.handle_thread_join(next, thread_id)?,
            }
            ecx.machine.threads.set_active_thread(old);
        }

        // Mark the corresponding StackState as not using (Best-Fit strategy)
        if let Some(stack_state) =
            ecx.machine.threads.stack_pool().iter_mut().find(|s| s.base_vaddr == stack_bottom)
        {
            assert!(
                stack_state.using,
                "{stack_state:?} must be used for stack_bottom={stack_bottom:#x}"
            );
            stack_state.clear_using();
        }

        interp_ok(())
    }

    /// Block the current thread, with an optional timeout.
    /// The callback will be invoked when the thread gets unblocked.
    #[inline]
    fn block_thread(
        &mut self,
        reason: BlockReason,
        deadline: Option<Deadline>,
        callback: DynUnblockCallback<'tcx>,
    ) {
        let ecx = self.eval_context_mut();
        if deadline.is_some() && ecx.machine.data_race.as_genmc_ref().is_some() {
            panic!("Unimplemented: Timeouts not yet supported in GenMC mode.");
        }
        if matches!(deadline, Some(Deadline::RealTime(_))) && !ecx.machine.communicate() {
            panic!("cannot have `RealTime` timeout with isolation");
        }
        ecx.machine.threads.block_thread(reason, deadline, callback);
    }

    /// Put the blocked thread into the enabled state.
    /// Sanity-checks that the thread previously was blocked for the right reason.
    fn unblock_thread(&mut self, thread: ThreadId, reason: BlockReason) -> InterpResult<'tcx> {
        let ecx = self.eval_context_mut();
        let old_state =
            mem::replace(&mut ecx.machine.threads.threads[thread].state, ThreadState::Enabled);
        let callback = match old_state {
            ThreadState::Blocked { reason: actual_reason, callback, .. } => {
                assert_eq!(
                    reason, actual_reason,
                    "unblock_thread: thread was blocked for the wrong reason"
                );
                callback
            }
            _ => panic!("unblock_thread: thread was not blocked"),
        };
        // The callback must be executed in the previously blocked thread.
        let old_thread = ecx.machine.threads.set_active_thread(thread);
        callback.call(ecx, UnblockKind::Ready)?;
        ecx.machine.threads.set_active_thread(old_thread);
        interp_ok(())
    }

    /// Find all threads with expired timeouts, unblock them and execute their timeout callbacks.
    ///
    /// This method returns the minimum duration until the next thread deadline.
    /// If all ready threads have no deadline set, [`None`] is returned.
    fn unblock_expired_deadlines(&mut self) -> InterpResult<'tcx, Option<Duration>> {
        let ecx = self.eval_context_mut();
        let communicate = ecx.machine.communicate();

        let mut min_wait_time = Option::<Duration>::None;
        let mut callbacks = Vec::new();

        for (id, thread) in ecx.machine.threads.threads.iter_enumerated_mut() {
            match &thread.state {
                ThreadState::Blocked { deadline: Some(deadline), .. } => {
                    let wait_time = match deadline {
                        Deadline::Monotonic(instant) =>
                            instant.duration_since(ecx.machine.monotonic_clock.now()),
                        Deadline::RealTime(time) => {
                            assert!(communicate, "cannot have `RealTime` timeout with isolation");
                            time.duration_since(SystemTime::now()).unwrap_or(Duration::ZERO)
                        }
                    };

                    if wait_time.is_zero() {
                        // The timeout expired for this thread.
                        let old_state = mem::replace(&mut thread.state, ThreadState::Enabled);
                        let ThreadState::Blocked { callback, .. } = old_state else {
                            unreachable!()
                        };
                        // Add callback to list to be run after this loop because of borrow-checking.
                        callbacks.push((id, callback));
                    } else {
                        // Update `min_wait_time` to contain the smallest duration until
                        // the next timeout expires.
                        min_wait_time = Some(wait_time.min(min_wait_time.unwrap_or(Duration::MAX)));
                    }
                }
                _ => {}
            }
        }

        for (thread, callback) in callbacks {
            // This back-and-forth with `set_active_thread` is here because of two
            // design decisions:
            // 1. Make the caller and not the callback responsible for changing
            //    thread.
            // 2. Make the scheduler the only place that can change the active
            //    thread.
            let old_thread = ecx.machine.threads.set_active_thread(thread);
            callback.call(ecx, UnblockKind::TimedOut)?;
            ecx.machine.threads.set_active_thread(old_thread);
        }

        interp_ok(min_wait_time)
    }

    #[inline]
    fn detach_thread(
        &mut self,
        thread_id: ThreadId,
        allow_terminated_joined: bool,
    ) -> InterpResult<'tcx> {
        let ecx = self.eval_context_mut();
        ecx.machine.threads.detach_thread(thread_id, allow_terminated_joined)
    }

    /// Mark that the active thread tries to join the thread with `joined_thread_id`.
    ///
    /// When the join is successful (immediately, or as soon as the joined thread finishes), `success_retval` will be written to `return_dest`.
    fn join_thread(
        &mut self,
        joined_thread_id: ThreadId,
        success_retval: Scalar,
        return_dest: &MPlaceTy<'tcx>,
    ) -> InterpResult<'tcx> {
        let ecx = self.eval_context_mut();
        let thread_mgr = &mut ecx.machine.threads;
        if thread_mgr.threads[joined_thread_id].join_status == ThreadJoinStatus::Detached {
            // On Windows this corresponds to joining on a closed handle.
            throw_ub_format!("trying to join a detached thread");
        }

        fn after_join<'tcx>(
            ecx: &mut MiriInterpCx<'tcx>,
            joined_thread_id: ThreadId,
            success_retval: Scalar,
            return_dest: &MPlaceTy<'tcx>,
        ) -> InterpResult<'tcx> {
            let threads = &ecx.machine.threads;
            match &mut ecx.machine.data_race {
                GlobalDataRaceHandler::None => {}
                GlobalDataRaceHandler::Vclocks(data_race) =>
                    data_race.thread_joined(threads, joined_thread_id),
                GlobalDataRaceHandler::Genmc(genmc_ctx) =>
                    genmc_ctx.handle_thread_join(threads.active_thread, joined_thread_id)?,
            }
            ecx.write_scalar(success_retval, return_dest)?;
            interp_ok(())
        }

        // Mark the joined thread as being joined so that we detect if other
        // threads try to join it.
        thread_mgr.threads[joined_thread_id].join_status = ThreadJoinStatus::Joined;
        if !thread_mgr.threads[joined_thread_id].state.is_terminated() {
            trace!(
                "{:?} blocked on {:?} when trying to join",
                thread_mgr.active_thread, joined_thread_id
            );
            if let Some(genmc_ctx) = ecx.machine.data_race.as_genmc_ref() {
                genmc_ctx.handle_thread_join(thread_mgr.active_thread, joined_thread_id)?;
            }

            // The joined thread is still running, we need to wait for it.
            // Once we get unblocked, perform the appropriate synchronization and write the return value.
            let dest = return_dest.clone();
            thread_mgr.block_thread(
                BlockReason::Join(joined_thread_id),
                None,
                callback!(
                    @capture<'tcx> {
                        joined_thread_id: ThreadId,
                        dest: MPlaceTy<'tcx>,
                        success_retval: Scalar,
                    }
                    |ecx, unblock: UnblockKind| {
                        assert_eq!(unblock, UnblockKind::Ready);
                        after_join(ecx, joined_thread_id, success_retval, &dest)
                    }
                ),
            );
        } else {
            // The thread has already terminated - establish happens-before and write the return value.
            after_join(ecx, joined_thread_id, success_retval, return_dest)?;
        }
        interp_ok(())
    }

    /// Mark that the active thread tries to exclusively join the thread with `joined_thread_id`.
    /// If the thread is already joined by another thread, it will throw UB.
    ///
    /// When the join is successful (immediately, or as soon as the joined thread finishes), `success_retval` will be written to `return_dest`.
    fn join_thread_exclusive(
        &mut self,
        joined_thread_id: ThreadId,
        success_retval: Scalar,
        return_dest: &MPlaceTy<'tcx>,
    ) -> InterpResult<'tcx> {
        let ecx = self.eval_context_mut();
        let threads = &ecx.machine.threads.threads;
        if threads[joined_thread_id].join_status == ThreadJoinStatus::Joined {
            throw_ub_format!("trying to join an already joined thread");
        }

        if joined_thread_id == ecx.machine.threads.active_thread {
            throw_ub_format!("trying to join itself");
        }

        // Sanity check `join_status`.
        assert!(
            threads
                .iter()
                .all(|thread| { !thread.state.is_blocked_on(BlockReason::Join(joined_thread_id)) }),
            "this thread already has threads waiting for its termination"
        );

        ecx.join_thread(joined_thread_id, success_retval, return_dest)
    }

    #[inline]
    fn active_thread(&self) -> ThreadId {
        let ecx = self.eval_context_ref();
        ecx.machine.threads.active_thread()
    }

    #[inline]
    fn active_thread_mut(&mut self) -> &mut Thread<'tcx> {
        let ecx = self.eval_context_mut();
        ecx.machine.threads.active_thread_mut()
    }

    #[inline]
    fn active_thread_ref(&self) -> &Thread<'tcx> {
        let ecx = self.eval_context_ref();
        ecx.machine.threads.active_thread_ref()
    }

    #[inline]
    fn get_total_thread_count(&self) -> usize {
        let ecx = self.eval_context_ref();
        ecx.machine.threads.get_total_thread_count()
    }

    #[inline]
    fn have_all_terminated(&self) -> bool {
        let ecx = self.eval_context_ref();
        ecx.machine.threads.have_all_terminated()
    }

    #[inline]
    fn enable_thread(&mut self, thread_id: ThreadId) {
        let ecx = self.eval_context_mut();
        ecx.machine.threads.enable_thread(thread_id);
    }

    #[inline]
    fn active_thread_stack<'a>(&'a self) -> &'a [Frame<'tcx, Provenance, FrameExtra<'tcx>>] {
        let ecx = self.eval_context_ref();
        ecx.machine.threads.active_thread_stack()
    }

    #[inline]
    fn active_thread_stack_mut<'a>(
        &'a mut self,
    ) -> &'a mut Vec<Frame<'tcx, Provenance, FrameExtra<'tcx>>> {
        let ecx = self.eval_context_mut();
        ecx.machine.threads.active_thread_stack_mut()
    }

    /// Set the name of the current thread. The buffer must not include the null terminator.
    #[inline]
    fn set_thread_name(&mut self, thread: ThreadId, new_thread_name: Vec<u8>) {
        self.eval_context_mut().machine.threads.set_thread_name(thread, new_thread_name);
    }

    #[inline]
    fn get_thread_name<'c>(&'c self, thread: ThreadId) -> Option<&'c [u8]>
    where
        'tcx: 'c,
    {
        self.eval_context_ref().machine.threads.get_thread_name(thread)
    }

    #[inline]
    fn yield_active_thread(&mut self) {
        // We do not yield immediately, as swapping out the current stack while executing a MIR statement
        // could lead to all sorts of confusion.
        // We should only switch stacks between steps.
        self.eval_context_mut().machine.threads.yield_active_thread = true;
    }

    #[inline]
    fn maybe_preempt_active_thread(&mut self) {
        let ecx = self.eval_context_mut();
        if !ecx.machine.threads.fixed_scheduling
            && ecx.machine.rng.get_mut().random_bool(ecx.machine.preemption_rate)
        {
            ecx.yield_active_thread();
        }
    }

    /// At the end of a basic block, moves execution to another running CPU with
    /// probability `[smp].switch_rate`. Draws nothing while only one CPU runs, so
    /// single-CPU runs see the same random choices as before.
    fn maybe_switch_cpu(&mut self) {
        let this = self.eval_context_mut();
        let rate = this.machine.smp_config.switch_rate();
        if rate == 0.0
            || this.machine.threads.num_cpus() == 1
            || this.machine.threads.next_cpu.is_some()
        {
            return;
        }
        let others = this.machine.threads.other_runnable_cpus();
        if !others.is_empty() && this.machine.smp_rng.random_bool(rate) {
            this.machine.threads.next_cpu = others.into_iter().choose(&mut this.machine.smp_rng);
        }
    }

    /// A CPU context yielded (for example a spin-loop hint): let another running CPU,
    /// chosen at random, take the next step. With no other running CPU it continues.
    fn yield_to_other_cpu(&mut self) {
        let this = self.eval_context_mut();
        let others = this.machine.threads.other_runnable_cpus();
        if let Some(cpu) = others.into_iter().choose(&mut this.machine.smp_rng) {
            this.machine.threads.next_cpu = Some(cpu);
        }
    }
}
