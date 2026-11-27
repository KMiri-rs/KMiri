//@compile-flags: -Zkmiri-toml=dev/test_utils/clint.toml -Zmiri-permissive-provenance
//! The CLINT timer interrupt is a level: it is pending once `mtime` reaches
//! the CPU's compare value, and the handler is entered again until it moves
//! the compare value past `mtime`.

#[path = "../../../dev/test_utils/clint.rs"]
mod clint;
#[path = "../../../dev/test_utils/irq.rs"]
mod irq;

use std::sync::atomic::{AtomicUsize, Ordering};

static ENTRIES: AtomicUsize = AtomicUsize::new(0);

extern "Rust" fn handler(vector: u8) {
    assert_eq!(vector, clint::TIMER);
    // Leave the level pending twice.
    if ENTRIES.fetch_add(1, Ordering::SeqCst) == 2 {
        clint::set_mtimecmp(0, u64::MAX);
    }
}

fn main() {
    irq::register_entry(handler);
    // No timer interrupt until the program sets one.
    assert_eq!(clint::mtimecmp(0), u64::MAX);
    let start = clint::mtime();
    for _ in 0..10 {
        std::hint::black_box(());
    }
    // Time advances with executed basic blocks.
    assert!(clint::mtime() > start);
    clint::set_mtimecmp(0, clint::mtime() + 50);
    irq::enable();
    while ENTRIES.load(Ordering::SeqCst) < 3 {
        std::hint::black_box(());
    }
    for _ in 0..100 {
        std::hint::black_box(());
    }
    assert_eq!(ENTRIES.load(Ordering::SeqCst), 3);
}
