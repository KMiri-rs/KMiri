// Statics that Rust allows to change must stay writable in KMiri's pseudo physical memory:
// a `static mut`, and a `static` whose type has interior mutability (`UnsafeCell`).
// Counterpart of `tests/fail/kmiri/write-immutable-static.rs`.

use std::cell::UnsafeCell;

struct Shared(UnsafeCell<usize>);
// SAFETY: this test is single-threaded.
unsafe impl Sync for Shared {}

static SHARED: Shared = Shared(UnsafeCell::new(1));
static mut PLAIN: usize = 1;

fn main() {
    let shared = SHARED.0.get();
    let plain = &raw mut PLAIN;
    unsafe {
        shared.write(2);
        plain.write(3);
        assert_eq!(shared.read(), 2);
        assert_eq!(plain.read(), 3);
    }
}
