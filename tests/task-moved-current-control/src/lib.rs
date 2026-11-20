//! Control for `task-moved-current`: the same client scheduler moves the
//! suspended task to a new allocation, but the task fetches `Task::current()`
//! again after it resumes. KMiri must resume the moved task and report nothing.
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
    let before = Task::current().unwrap().schedule_info().cpu.get().is_some();
    note!("A: before yield, cpu set = {}", before);
    Task::yield_now();
    // Correct use: fetch the current task again after the yield.
    let after = Task::current().unwrap().schedule_info().cpu.get().is_some();
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
