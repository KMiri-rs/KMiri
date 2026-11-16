//! Control for `tests/irq-rcu-uaf`: the same driver/peer sequence, but the RCU
//! read reference and both IRQ guards are released in LIFO order before
//! `update`. No reference to the old payload is alive when it is reclaimed, so
//! KMiri must report nothing.
#![no_std]
#![deny(unsafe_code)]
#![feature(format_args_nl, stmt_expr_attributes)]

extern crate alloc;

use alloc::boxed::Box;
use core::sync::atomic::{AtomicBool, Ordering};
use ostd::{
    sync::Rcu,
    task::{Task, TaskOptions},
};

static PEER_READY: AtomicBool = AtomicBool::new(false);
static STOP_PEER: AtomicBool = AtomicBool::new(false);
static DRIVER_DONE: AtomicBool = AtomicBool::new(false);
static OLD_RECLAIMED: AtomicBool = AtomicBool::new(false);

struct Payload {
    value: u64,
}

impl Drop for Payload {
    fn drop(&mut self) {
        if self.value == 0xAA {
            OLD_RECLAIMED.store(true, Ordering::SeqCst);
        }
    }
}

#[ostd::ktest::miri_main]
fn kernel_main() {
    let _driver = TaskOptions::new(|| {
        let _peer = TaskOptions::new(|| {
            PEER_READY.store(true, Ordering::SeqCst);
            while !STOP_PEER.load(Ordering::SeqCst) {
                Task::yield_now();
            }
        })
        .spawn()
        .unwrap();

        while !PEER_READY.load(Ordering::SeqCst) {
            Task::yield_now();
        }

        let rcu = Rcu::new(Box::new(Payload { value: 0xAA }));
        let outer = ostd::irq::disable_local();
        let inner = ostd::irq::disable_local();
        let old = rcu.read_with(&inner);
        let value = old.deref_target().value;

        // Correct LIFO order: the read reference ends before IRQs come back.
        drop(old);
        drop(inner);
        drop(outer);
        assert!(ostd::irq::miri_local_irqs_enabled());
        rcu.update(Box::new(Payload { value: 0xBB }));

        for _ in 0..128 {
            if OLD_RECLAIMED.load(Ordering::SeqCst) {
                break;
            }
            Task::yield_now();
        }
        assert!(
            OLD_RECLAIMED.load(Ordering::SeqCst),
            "RCU grace period did not reclaim the old payload"
        );

        STOP_PEER.store(true, Ordering::SeqCst);
        Task::yield_now();

        ostd::miri_println!("irq-rcu-uaf-lifo: reclaimed, value read before reclaim = {:#x}", value);
        DRIVER_DONE.store(true, Ordering::SeqCst);
    })
    .spawn()
    .unwrap();

    while !DRIVER_DONE.load(Ordering::SeqCst) {
        Task::yield_now();
    }
}
