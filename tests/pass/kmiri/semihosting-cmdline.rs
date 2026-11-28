//@compile-flags: -Zkmiri-toml=dev/test_utils/no-paging.toml

//! The semihosting command line is the program arguments, the program name
//! first, joined by spaces; a buffer too small for it and its NUL fails.

use std::ffi::c_void;
use std::ptr::without_provenance_mut as int;

unsafe extern "C" {
    fn kern_miri_semihosting(op: usize, param: *mut c_void) -> *mut c_void;
}

const SYS_GET_CMDLINE: usize = 0x15;

fn get_cmdline(buf: &mut [u8]) -> Result<usize, ()> {
    let mut block: [*mut c_void; 2] = [buf.as_mut_ptr().cast(), int(buf.len())];
    let ret = unsafe { kern_miri_semihosting(SYS_GET_CMDLINE, block.as_mut_ptr().cast()) };
    if ret.addr() == 0 {
        Ok(block[1].addr())
    } else {
        assert_eq!(ret.addr(), usize::MAX, "a failed request returns -1");
        Err(())
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let expected = args.join(" ");

    let mut buf = [0xffu8; 512];
    let len = get_cmdline(&mut buf).unwrap();
    assert_eq!(&buf[..len], expected.as_bytes());
    assert_eq!(buf[len], 0);

    // Room for the text but not its NUL.
    let mut small = vec![0u8; expected.len()];
    assert_eq!(get_cmdline(&mut small), Err(()));
}
