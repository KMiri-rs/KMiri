//@compile-flags: -Zkmiri-toml=dev/test_utils/pool.toml -Zmiri-permissive-provenance
//! Freeing the object that holds the stack of the running thread is reported.

unsafe extern "Rust" {
    fn kern_miri_object_born(ptr: *mut u8, size: usize, align: usize) -> *mut u8;
    fn kern_miri_object_died(ptr: *mut u8) -> *mut u8;
}

const POOL: usize = 0x2000000;

/// The block at `POOL + offset`, reached through its address as an allocator does.
fn block(offset: usize) -> *mut u8 {
    std::ptr::with_exposed_provenance_mut(POOL + offset)
}

unsafe extern "Rust" {
    fn miri_create_new_thread(
        func: fn(usize),
        arg: usize,
        stack_end: usize,
        stack_size: usize,
    ) -> usize;
    fn miri_switch_to(thread: usize);
}

fn entry(stack: usize) {
    unsafe { kern_miri_object_died(std::ptr::with_exposed_provenance_mut(stack)) }; //~ ERROR: holds the stack of a thread running on a CPU
}

fn main() {
    unsafe {
        let stack = kern_miri_object_born(block(0), 0x10000, 16);
        let thread = miri_create_new_thread(
            entry,
            stack.expose_provenance(),
            stack.addr() + 0x10000,
            0x10000,
        );
        miri_switch_to(thread);
    }
}
