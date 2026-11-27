//@compile-flags: -Zkmiri-toml=dev/test_utils/no-paging.toml

//! The semihosting console: open `:tt`, write a block, write a C string,
//! then a normal exit stops the program with the given status.

use std::ffi::c_void;
use std::ptr::without_provenance_mut as int;

unsafe extern "C" {
    fn kern_miri_semihosting(op: usize, param: *mut c_void) -> *mut c_void;
}

const SYS_OPEN: usize = 0x01;
const SYS_WRITE0: usize = 0x04;
const SYS_WRITE: usize = 0x05;
const SYS_EXIT: usize = 0x18;

fn call(op: usize, param: *mut c_void) -> usize {
    unsafe { kern_miri_semihosting(op, param) }.addr()
}

fn main() {
    // Parameter blocks hold pointers and numbers, like the result register.
    let name = b":tt\0";
    let mut open: [*mut c_void; 3] = [name.as_ptr().cast_mut().cast(), int(4), int(3)];
    let stdout = call(SYS_OPEN, open.as_mut_ptr().cast());

    let msg = b"block write\n";
    let mut write: [*mut c_void; 3] = [int(stdout), msg.as_ptr().cast_mut().cast(), int(msg.len())];
    assert_eq!(call(SYS_WRITE, write.as_mut_ptr().cast()), 0);

    call(SYS_WRITE0, c"c string\n".as_ptr().cast_mut().cast());

    let mut exit: [*mut c_void; 2] = [int(0x20026), int(0)];
    call(SYS_EXIT, exit.as_mut_ptr().cast());
    unreachable!("semihosting exit returned");
}
