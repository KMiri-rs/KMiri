//@compile-flags: -Zmiri-disable-stacked-borrows
// A `static` without interior mutability is read-only: writing to it is undefined behavior
// however the pointer was obtained. KMiri copies every static into its pseudo physical memory;
// the copy must stay read-only, or such writes go unreported (Asterinas issue #3871: assembly
// stores into `TSS.sp0` of an immutable `static LOCAL_TSS`).
//
// Stacked Borrows is disabled so that only the allocation's mutability can report the write:
// an address coming from assembly carries no borrow tag, so the aliasing model does not catch
// that case either.

static VALUE: usize = 1;

fn main() {
    let ptr = &raw const VALUE as *mut usize;
    unsafe { ptr.write(2) }; //~ ERROR: read-only
}
