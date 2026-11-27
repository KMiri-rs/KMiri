//! This file contains Miri's scheduler, which is in charge of picking the next thread to run when
//! the current thread blocks/yields. It also manages timeouts and runs an async event loop
//! to deal with host I/O. In GenMC mode, it handles delegating schedule control to GenMC.

use std::cell::RefCell;
use std::fmt;
use std::sync::atomic::Ordering;
use std::task::Poll;
use std::time::Duration;

use rand::RngExt;
use rand::seq::IteratorRandom;
use rustc_abi::ExternAbi;
use rustc_const_eval::CTRL_C_RECEIVED;
use rustc_index::Idx;
use rustc_middle::mir;
use rustc_span::DUMMY_SP;

use crate::mirch::PageState;
use crate::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SchedulingAction {
    /// Execute step on the active thread.
    ExecuteStep,
    /// Wait for a bit, but at most as long as the duration specified.
    /// We wake up early if an I/O event happened.
    /// If the duration is [`None`], we sleep indefinitely. This is
    /// only allowed when isolation is disabled and there are threads waiting for I/O!
    SleepAndWaitForIo(Option<Duration>),
}

/// When we need to perform an operation but don't have an `ecx` around to run it immediately, we
/// can register a callback here and it will be invoked on the next scheduler call.
#[derive(Default)]
pub struct DelayedCallbacks {
    callbacks: RefCell<Vec<DelayedCallback>>,
}

pub type DelayedCallback = Box<dyn for<'tcx> FnOnce(&mut MiriInterpCx<'tcx>) -> InterpResult<'tcx>>;

impl fmt::Debug for DelayedCallbacks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DelayedCallbacks").field("callbacks", &"_").finish()
    }
}

impl DelayedCallbacks {
    /// Register a callback to be invoked the next time the current thread yields/blocks.
    pub fn add(&self, callback: DelayedCallback) {
        self.callbacks.borrow_mut().push(callback);
    }

    fn process<'tcx>(ecx: &mut MiriInterpCx<'tcx>) -> InterpResult<'tcx> {
        loop {
            // Avoid keeping the RefCell open over the callback.
            let Some(callback) = ecx.machine.delayed_callbacks.callbacks.borrow_mut().pop() else {
                return interp_ok(());
            };
            (callback)(ecx)?;
        }
    }
}

impl<'tcx> EvalContextPrivExt<'tcx> for MiriInterpCx<'tcx> {}
trait EvalContextPrivExt<'tcx>: MiriInterpCxExt<'tcx> {
    #[inline]
    fn run_on_stack_empty(&mut self) -> InterpResult<'tcx, Poll<()>> {
        let ecx = self.eval_context_mut();
        let active_thread = ecx.active_thread_mut();
        active_thread.origin_span = DUMMY_SP; // reset, the old value no longer applies
        let mut callback = active_thread
            .on_stack_empty
            .take()
            .expect("`on_stack_empty` not set up, or already running");
        let res = callback(ecx)?;
        ecx.active_thread_mut().on_stack_empty = Some(callback);
        interp_ok(res)
    }

    /// Decide which action to take next and on which thread.
    ///
    /// The currently implemented scheduling policy is the one that is commonly
    /// used in stateless model checkers such as Loom: run the active thread as
    /// long as we can and switch only when we have to (the active thread was
    /// blocked, terminated, or has explicitly asked to be preempted).
    ///
    /// If GenMC mode is active, the scheduling is instead handled by GenMC.
    fn schedule(&mut self) -> InterpResult<'tcx, SchedulingAction> {
        let ecx = self.eval_context_mut();

        // A simulated IRQ runs synchronously on the interrupted thread and CPU, so
        // no other context of this CPU runs until its handler returns. Other CPUs
        // keep running meanwhile: a yield in the handler (such as a spin-loop hint)
        // lets another CPU run, and random CPU switches apply.
        let cpu = ecx.machine.threads.active_cpu;
        if ecx.machine.irq_cpus[cpu].active.is_some() {
            if std::mem::take(&mut ecx.machine.threads.yield_active_thread) {
                ecx.yield_to_other_cpu();
            }
            ecx.machine.threads.apply_cpu_switch();
            return interp_ok(SchedulingAction::ExecuteStep);
        }

        // In GenMC mode, we let GenMC do the scheduling.
        if ecx.machine.data_race.as_genmc_ref().is_some() {
            loop {
                let genmc_ctx = ecx.machine.data_race.as_genmc_ref().unwrap();
                let Some(next_thread_id) = genmc_ctx.schedule_thread(ecx)? else {
                    return interp_ok(SchedulingAction::ExecuteStep);
                };
                // If a thread is blocked on GenMC, we have to implicitly unblock it when it gets scheduled again.
                if ecx.machine.threads.thread_ref(next_thread_id).is_blocked_on(BlockReason::Genmc)
                {
                    info!(
                        "GenMC: scheduling blocked thread {next_thread_id:?}, so we unblock it now."
                    );
                    ecx.unblock_thread(next_thread_id, BlockReason::Genmc)?;
                }
                // The thread we just unblocked may have been blocked again during the unblocking callback.
                // In that case, we need to ask for a different thread to run next.
                let thread_manager = &mut ecx.machine.threads;
                if thread_manager.thread_ref(next_thread_id).is_enabled() {
                    // Set the new active thread.
                    thread_manager.set_active_thread(next_thread_id);
                    return interp_ok(SchedulingAction::ExecuteStep);
                }
            }
        }

        // We are not in GenMC mode, so we control the scheduling.
        let thread_manager = &mut ecx.machine.threads;

        // kmiri: thread and cpu switch
        if let Some(res) = thread_manager.schedule_switch_thread_and_cpu() {
            return res;
        }

        if ecx.machine.threads.is_cpu_halted(ecx.machine.threads.active_cpu) {
            if !ecx.machine.threads.other_runnable_cpus().is_empty() {
                ecx.yield_to_other_cpu();
                ecx.machine.threads.apply_cpu_switch();
                return interp_ok(SchedulingAction::ExecuteStep);
            }
            let timer_can_fire = ecx.machine.irq_entry.is_some()
                && ecx.machine.timer_vector.is_some()
                && (ecx.machine.irq_config.tick_interval_basic_blocks() != 0
                    || ecx.machine.irq_config.random_injection_rate() > 0.0
                    || ecx
                        .machine
                        .irq_config
                        .inject_at_basic_blocks()
                        .iter()
                        .any(|&b| b > ecx.machine.basic_block_count));
            if timer_can_fire {
                let halted: Vec<usize> = (0..ecx.machine.threads.num_cpus())
                    .filter(|&c| ecx.machine.threads.is_cpu_halted(c))
                    .collect();
                let idx = ecx.machine.irq_rng.random_range(0..halted.len());
                let cpu = halted[idx];
                let vector = ecx.machine.timer_vector.unwrap();
                let event_id = ecx.machine.queue_irq(cpu, vector, crate::machine::IrqSource::Timer);
                ecx.machine.threads.set_active_cpu(cpu);
                if ecx.machine.irq_config.trace() {
                    eprintln!(
                        "KMiri IRQ: event={event_id} cpu={cpu} action=timer-queued vector={vector} source=Timer block={} IF={} pending={} reason=all-halted",
                        ecx.machine.basic_block_count,
                        ecx.machine.irq_cpus[cpu].enabled,
                        ecx.machine.irq_cpus[cpu].pending.len(),
                    );
                }
                return interp_ok(SchedulingAction::ExecuteStep);
            }
            // Another CPU's context may be blocked (on a timeout or host I/O). Let Miri's
            // unblocking machinery run, but never step the halted CPU.
            DelayedCallbacks::process(ecx)?;
            if ecx.machine.communicate() {
                ecx.poll_and_unblock(Some(Duration::ZERO))?;
            }
            let sleep_time = ecx.unblock_expired_deadlines()?;
            if let Some(&cpu) = ecx.machine.threads.other_runnable_cpus().first() {
                ecx.machine.threads.set_active_cpu(cpu);
                return interp_ok(SchedulingAction::ExecuteStep);
            }
            if let Some(sleep_time) = sleep_time {
                return interp_ok(SchedulingAction::SleepAndWaitForIo(Some(sleep_time)));
            }
            if ecx.any_thread_blocked_on_host() {
                return interp_ok(SchedulingAction::SleepAndWaitForIo(None));
            }
            let any_blocked = (0..ecx.machine.threads.num_cpus()).any(|c| {
                !ecx.machine.threads.is_cpu_halted(c)
                    && ecx.machine.threads.cpu_thread(c).is_some()
                    && !ecx.machine.threads.is_cpu_thread_enabled(c)
            });
            if any_blocked {
                throw_machine_stop!(TerminationInfo::GlobalDeadlock);
            }
            {
                let halted = (0..ecx.machine.threads.num_cpus())
                    .filter(|&c| ecx.machine.threads.is_cpu_halted(c))
                    .map(|c| {
                        let name = ecx
                            .machine
                            .threads
                            .cpu_thread(c)
                            .map(|t| ecx.machine.threads.get_thread_display_name(t))
                            .unwrap_or_else(|| "<none>".to_owned());
                        (c, name)
                    })
                    .collect();
                throw_machine_stop!(TerminationInfo::AllCpusHalted { halted });
            }
        }

        // kmiri: kernel contexts are resumed only by the kernel's own switches, never
        // picked by the scheduler below. A yield of a CPU context (a spin-loop hint or
        // a preemption point) lets another CPU run instead.
        if ecx.machine.threads.yield_active_thread
            && !ecx.machine.kernel_threads.is_empty()
            && ecx.machine.threads.active_thread_ref().is_enabled()
        {
            ecx.machine.threads.yield_active_thread = false;
            ecx.yield_to_other_cpu();
            ecx.machine.threads.apply_cpu_switch();
            return interp_ok(SchedulingAction::ExecuteStep);
        }
        let thread_manager = &mut ecx.machine.threads;

        // Check if we can just keep running the current thread.
        if thread_manager.active_thread_ref().is_enabled() && !thread_manager.yield_active_thread {
            // The currently active thread is still enabled, just continue with it.
            return interp_ok(SchedulingAction::ExecuteStep);
        }

        // The active thread yielded or got terminated. Let's see if there are any I/O events
        // or timeouts to take care of.

        // There may be delayed callbacks we have to process, which may unblock more threads.
        DelayedCallbacks::process(ecx)?;

        if ecx.machine.communicate() {
            // When isolation is disabled we need to check for events for threads
            // which are blocked on host I/O. Unlike the `poll_and_unblock` before
            // any foreign item, the call here is needed to ensure that threads which
            // are blocked on host I/O are woken up even if no shimmed functions are
            // executed afterwards.
            // We do this before running any other threads such that the threads
            // which received events are available for scheduling afterwards.

            // Perform a non-blocking poll for newly available I/O events from the OS.
            ecx.poll_and_unblock(Some(Duration::ZERO))?;
        }

        // We also check timeouts before running any other thread, to ensure that timeouts
        // "in the past" fire before any other thread can take an action. This ensures that for
        // `pthread_cond_timedwait`, "an error is returned if [...] the absolute time specified by
        // abstime has already been passed at the time of the call".
        // <https://pubs.opengroup.org/onlinepubs/9699919799/functions/pthread_cond_timedwait.html>
        let potential_sleep_time = ecx.unblock_expired_deadlines()?;

        let thread_manager = &mut ecx.machine.threads;
        let rng = ecx.machine.rng.get_mut();

        // No callbacks immediately scheduled, pick a regular thread to execute.
        // The active thread blocked or yielded. So we go search for another enabled thread.
        // We build the list of threads by starting with the threads after the current one, followed by
        // the threads before the current one and then the current thread itself (i.e., this iterator acts
        // like `threads.rotate_left(self.active_thread.index() + 1)`. This ensures that if we pick the first
        // eligible thread, we do regular round-robin scheduling, and all threads get a chance to take a step.
        let mut threads_iter = thread_manager
            .all_threads()
            .skip(thread_manager.active_thread().index() + 1)
            .chain(thread_manager.all_threads().take(thread_manager.active_thread().index() + 1))
            .filter(|(id, thread)| {
                thread.is_enabled() && !thread_manager.is_halted_cpu_context(*id)
            });
        // Pick a new thread, and switch to it.
        let new_thread = if thread_manager.fixed_scheduling() {
            let next = threads_iter.next();
            drop(threads_iter);
            next
        } else {
            threads_iter.choose(rng)
        };

        if let Some((id, _thread)) = new_thread {
            if thread_manager.active_thread() != id {
                thread_manager.set_active_thread(id);
            }
        }
        // This completes the `yield`, if any was requested.
        thread_manager.yield_active_thread = false;

        if thread_manager.active_thread_ref().is_enabled() {
            return interp_ok(SchedulingAction::ExecuteStep);
        }

        // We have not found a thread to execute.
        if ecx.machine.threads.all_threads().all(|(_id, thread)| thread.is_terminated()) {
            unreachable!("all threads terminated without the main thread terminating?!");
        } else if let Some(sleep_time) = potential_sleep_time {
            // All threads are currently blocked, but we have unexecuted
            // timeout_callbacks, which may unblock some of the threads. Hence,
            // sleep until the first callback.
            interp_ok(SchedulingAction::SleepAndWaitForIo(Some(sleep_time)))
        } else if ecx.any_thread_blocked_on_host() {
            // At least one thread doesn't have a timeout set, and is blocked on host I/O or is waiting on an
            // epoll instance which contains a host source interest. Hence, we sleep indefinitely in the
            // hope that eventually an I/O event happens.
            interp_ok(SchedulingAction::SleepAndWaitForIo(None))
        } else {
            throw_machine_stop!(TerminationInfo::GlobalDeadlock);
        }
    }
}

impl<'tcx> EvalContextExt<'tcx> for crate::MiriInterpCx<'tcx> {}
pub trait EvalContextExt<'tcx>: crate::MiriInterpCxExt<'tcx> {
    /// Public because this is used by Priroda.
    fn step_current_thread(&mut self) -> InterpResult<'tcx> {
        let ecx = self.eval_context_mut();
        let basic_blocks_before = ecx.machine.basic_block_count;
        let cpu = ecx.machine.threads.active_cpu;

        if !ecx.step()? {
            // The stack is empty. See if this thread can do something else.
            match ecx.run_on_stack_empty()? {
                Poll::Pending => {} // keep going
                Poll::Ready(()) => {
                    ecx.terminate_active_thread(/* for_exit */ false)?;
                }
            }
        }

        let blocks_executed = ecx.machine.basic_block_count - basic_blocks_before;
        let current_stack_depth = ecx.active_thread_stack().len();
        if let Some((event_id, vector, source, was_enabled)) =
            ecx.machine.finish_irq_if_returned(cpu, current_stack_depth)
        {
            if ecx.machine.irq_config.trace() {
                eprintln!(
                    "KMiri IRQ: event={event_id} cpu={cpu} action=returned vector={vector} source={source:?} block_count={} IF={}",
                    ecx.machine.basic_block_count, ecx.machine.irq_cpus[cpu].enabled
                );
            }
            debug_assert_eq!(ecx.machine.irq_cpus[cpu].enabled, was_enabled);
        }

        // `before_terminator` queues timer events before the terminator executes.
        // Dispatch only after `step` completed, so the interrupted MIR frame is at
        // a stable block boundary and the handler returns to its successor block.
        let top_frame = ecx.active_thread_stack().last();
        if blocks_executed != 0
            && ecx.machine.irq_config.enabled()
            && ecx.machine.irq_cpus[cpu].active.is_none()
            && ecx.machine.irq_cpus[cpu].enabled
            && !ecx.machine.irq_cpus[cpu].pending.is_empty()
            && let Some(frame) = top_frame
            && let Some(loc) = frame.current_loc().left()
        {
            let pending = ecx.machine.irq_cpus[cpu].pending.pop_front().unwrap();
            if let Some(clock) = pending.clock {
                ecx.acquire_clock(&clock)?;
            }
            let interrupted_depth = ecx.active_thread_stack().len();
            let event_id = pending.event_id;
            let vector = pending.vector;
            let source = pending.source;
            let entry = ecx.machine.irq_entry.unwrap();
            let return_block = loc.block;
            ecx.machine.irq_cpus[cpu].active = Some(crate::machine::ActiveIrq {
                event_id,
                vector,
                source,
                stack_depth: interrupted_depth,
                was_enabled: true,
            });
            ecx.machine.irq_cpus[cpu].enabled = false;
            if ecx.machine.irq_config.trace() {
                eprintln!(
                    "KMiri IRQ: event={event_id} cpu={cpu} thread={:?} action=delivered vector={vector} source={source:?} global_block_count={} return_block={} pending={} IF=0",
                    ecx.machine.threads.active_thread(),
                    ecx.machine.basic_block_count,
                    return_block.index(),
                    ecx.machine.irq_cpus[cpu].pending.len(),
                );
            }
            ecx.call_function(
                entry,
                ExternAbi::Rust,
                &[ImmTy::from_uint(vector, ecx.machine.layouts.u8)],
                None,
                ReturnContinuation::Goto {
                    ret: Some(return_block),
                    unwind: mir::UnwindAction::Continue,
                },
            )?;
        }

        interp_ok(())
    }

    /// Run the core interpreter loop. Returns only when an interrupt occurs (an error or program
    /// termination).
    fn run_threads(&mut self) -> InterpResult<'tcx, !> {
        let ecx = self.eval_context_mut();
        loop {
            if CTRL_C_RECEIVED.load(Ordering::Relaxed) {
                ecx.machine.handle_abnormal_termination();
                throw_machine_stop!(TerminationInfo::Interrupted);
            }
            match ecx.schedule()? {
                SchedulingAction::ExecuteStep => {
                    if let Some(paddr) = ecx.machine.pt_checker {
                        let page_start_paddr = paddr & !(mirch::page_size() - 1);
                        if let PageState::Typed { .. } =
                            mirch::physical_mem().page_states[page_start_paddr / mirch::page_size()]
                        {
                            let _global_states = ecx.machine.alloc_addresses.borrow();
                            //..todo!()
                        }
                        ecx.machine.pt_checker = None;
                    }

                    ecx.step_current_thread()?;
                    if let Some(handle) = &ecx.machine.debugger {
                        let stack = ecx.active_thread_stack();
                        if !stack.is_empty() {
                            handle.send(ecx);

                            match handle.wait_for_command(ecx) {
                                debugger::Quit::No => (),
                                debugger::Quit::Yes => {
                                    ecx.machine.handle_abnormal_termination();
                                    throw_machine_stop!(TerminationInfo::Interrupted)
                                }
                                debugger::Quit::YesWithErr(err) => {
                                    use std::io::Write;
                                    let file = std::fs::File::create("miri-debugger.log").unwrap();
                                    writeln!(&file, "Debugger exited with error:\n{err}").unwrap();
                                    ecx.machine.handle_abnormal_termination();
                                    throw_machine_stop!(TerminationInfo::Interrupted)
                                }
                            }
                        }
                    }
                }
                SchedulingAction::SleepAndWaitForIo(duration) => {
                    if ecx.machine.communicate() {
                        // When we're running with isolation disabled, instead of
                        // strictly sleeping the duration we allow waking up
                        // early for I/O events from the OS.

                        ecx.poll_and_unblock(duration)?;
                    } else {
                        let duration = duration.expect(
                            "Infinite sleep should not be triggered when isolation is enabled",
                        );
                        ecx.machine.monotonic_clock.sleep(duration);
                    }
                }
            }
        }
    }
}
