//! A suspended task's `Task` value can be moved to a new `Arc` allocation by a
//! client scheduler (safe code: `Arc::try_unwrap` + `Arc::new`). Natively the
//! task still resumes, because its saved context moves with the `Task`. KMiri
//! must resume it too, and then report the use of a `CurrentTask` taken before
//! the move: it points to the freed old allocation (use-after-free).
//! Control: `task-moved-current-control`.
#![no_std]
#![deny(unsafe_code)]
#![cfg_attr(miri, feature(format_args_nl))]

extern crate alloc;

use alloc::{boxed::Box, collections::VecDeque, sync::Arc};
use core::{
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, Ordering},
};

use ostd::{
    cpu::CpuId,
    sync::SpinLock,
    task::{
        Task, TaskOptions,
        scheduler::{EnqueueFlags, LocalRunQueue, Scheduler, UpdateFlags, inject_scheduler},
    },
};

// Diagnostics only; compiled out outside Miri.
macro_rules! note {
    ($($t:tt)*) => { #[cfg(miri)] ostd::miri_println!($($t)*); };
}

static A_DONE: AtomicBool = AtomicBool::new(false);

struct State {
    current: Option<Arc<Task>>,
    queue: VecDeque<Arc<Task>>,
}

/// Moves every uniquely-owned queued task to a fresh allocation.
fn relocate(queue: &mut VecDeque<Arc<Task>>) {
    let old = core::mem::take(queue);
    for arc in old {
        let arc = match Arc::try_unwrap(arc) {
            Ok(task) => {
                // Occupy the slot that was just freed so that the moved task
                // cannot land on the same address.
                core::mem::forget(Arc::new(MaybeUninit::<Task>::uninit()));
                Arc::new(task)
            }
            Err(arc) => arc,
        };
        queue.push_back(arc);
    }
}

impl LocalRunQueue for State {
    fn current(&self) -> Option<&Arc<Task>> {
        self.current.as_ref()
    }

    fn update_current(&mut self, flags: UpdateFlags) -> bool {
        match flags {
            UpdateFlags::Tick => false,
            _ => !self.queue.is_empty(),
        }
    }

    fn try_pick_next(&mut self) -> Option<&Arc<Task>> {
        let next = self.queue.pop_front()?;
        if let Some(prev) = self.current.replace(next) {
            self.queue.push_back(prev);
        }
        self.current.as_ref()
    }

    fn dequeue_current(&mut self) -> Option<Arc<Task>> {
        self.current.take()
    }
}

struct Sched {
    state: SpinLock<State>,
}

impl Scheduler for Sched {
    fn enqueue(&self, runnable: Arc<Task>, _flags: EnqueueFlags) -> Option<CpuId> {
        let mut st = self.state.disable_irq().lock();
        relocate(&mut st.queue);
        st.queue.push_back(runnable);
        None
    }

    fn local_rq_with(&self, f: &mut dyn FnMut(&dyn LocalRunQueue)) {
        let g = self.state.disable_irq().lock();
        f(&*g);
    }

    fn mut_local_rq_with(&self, f: &mut dyn FnMut(&mut dyn LocalRunQueue)) {
        let mut g = self.state.disable_irq().lock();
        f(&mut *g);
    }
}

fn task_a() {
    // A `CurrentTask` obtained before the yield (a raw pointer, no refcount).
    let cur = Task::current().unwrap();
    let before = cur.schedule_info().cpu.get().is_some();
    note!("A: before yield, cpu set = {}", before);
    // A goes to the scheduler queue; task B runs and spawns a task, which makes
    // the scheduler move A to a new `Arc` allocation.
    Task::yield_now();
    // Stale `CurrentTask`: reads a field of the freed old allocation.
    let after = cur.schedule_info().cpu.get().is_some();
    note!("A: after yield, cpu set = {}", after);
    A_DONE.store(true, Ordering::SeqCst);
}

fn task_b() {
    // Calls `Scheduler::enqueue`, which relocates the suspended task A.
    let _ = TaskOptions::new(|| {}).spawn().unwrap();
}

#[cfg_attr(miri, ostd::ktest::miri_main)]
#[cfg_attr(not(miri), ostd::main)]
fn kernel_main() {
    let sched: &'static Sched = Box::leak(Box::new(Sched {
        state: SpinLock::new(State {
            current: None,
            queue: VecDeque::new(),
        }),
    }));
    inject_scheduler(sched);

    // The returned `Arc`s are dropped at once: the scheduler is the only owner.
    let _ = TaskOptions::new(task_a).spawn().unwrap();
    let _ = TaskOptions::new(task_b).spawn().unwrap();

    // From the bootstrap context this switches to A. KMiri resumes the
    // bootstrap context once no task is left to run.
    Task::yield_now();
    assert!(A_DONE.load(Ordering::SeqCst));
}
