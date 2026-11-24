use std::alloc::Layout;
use std::collections::{BTreeMap, BTreeSet};

use rustc_abi::{Align, Size};
use rustc_middle::mir::interpret::{CheckInAllocMsg, alloc_range};
use rustc_middle::ty::{self, Mutability};

use super::PageTable;
use super::config::*;
use crate::alloc::MiriAllocParams;
use crate::concurrency::VClock;
use crate::mirch::config;
use crate::*;

static mut PHYSICAL_MEM: PhysicalMemory = PhysicalMemory::empty();

/// Inits a page-based pseudo physical memory for the KernMiri.
///
/// The memory size and page size are defined in [`self::config`].
pub fn init_pseudo_physical_mem(config: PhysConfig, page_table_enabled: bool) {
    *physical_mem_mut() = PhysicalMemory::new(config, page_table_enabled);
}

/// Returns an immutable reference to `PhysicalMemory` instance.
pub fn physical_mem<'a>() -> &'a PhysicalMemory {
    #[allow(clippy::deref_addrof)]
    unsafe {
        &*(&raw const PHYSICAL_MEM)
    }
}

/// Returns a mutable reference to `PhysicalMemory` instance.
pub fn physical_mem_mut<'a>() -> &'a mut PhysicalMemory {
    #[allow(clippy::deref_addrof)]
    unsafe {
        &mut *(&raw mut PHYSICAL_MEM)
    }
}

/// Convert a physical address to a pointer that point to
/// the corresponding position of the simulated physical memory.
pub fn paddr_to_mem(paddr: usize) -> *mut u8 {
    unsafe { physical_mem().mem.add(paddr) }
}

/// Checks whether a pointer points to the simulated physical memory.
pub fn is_in_physical_mem(ptr: *const ()) -> bool {
    let physical_mem = physical_mem();
    #[allow(clippy::as_conversions)]
    (physical_mem.mem as usize..physical_mem.mem as usize + total_mem_size())
        .contains(&(ptr as usize))
}

pub fn is_in_cpu_local_section(paddr: usize) -> bool {
    (cpu_local_start_addr()..cpu_local_end_addr()).contains(&paddr)
}

/// Creates an `Allocation` at `paddr` with `layout`.
///
/// The `paddr` is the physical address in the OS. This method will
/// put the backend bytes of created allocation in the corresponding
/// position of the simulated physical memory.
pub fn create_allocation_at(
    paddr: usize,
    layout: Layout,
    params: MiriAllocParams,
) -> Allocation<Provenance, (), MiriAllocBytes> {
    unsafe {
        let start = paddr_to_mem(paddr);
        let buffer = std::slice::from_raw_parts(start, layout.size());
        let mut allocation = Allocation::<Provenance, (), MiriAllocBytes>::from_bytes(
            std::borrow::Cow::Borrowed(buffer),
            Align::from_bytes(layout.align() as u64).unwrap(),
            Mutability::Mut,
            params,
        );

        // Take the initialization state from the per-page shadows. An
        // allocation can span several pages (a large heap object), so walk
        // it one page at a time.
        let init_masks = &physical_mem().init_masks;
        let mut done = 0;
        while done < layout.size() {
            let offset = (paddr + done) % page_size();
            let chunk = (page_size() - offset).min(layout.size() - done);
            if let Some(mask_allocation) = init_masks.get(&(paddr + done - offset)) {
                let init_copy =
                    mask_allocation.init_mask().prepare_copy((offset..offset + chunk).into());
                allocation.init_mask_apply_copy(init_copy, (done..done + chunk).into(), 1);
            }
            done += chunk;
        }
        allocation
    }
}

#[derive(Debug)]
pub struct KernelMem {
    pub alloc_id: AllocId,
    #[allow(unused)]
    pub provenance: Vec<String>,
    pub size: u64,
    pub align: u64,
    pub buffer_ptr: *const u8,
}

pub fn free_kernel_allocations(ecx: &mut MiriInterpCx<'_>, v_kernel_mem: Vec<KernelMem>) {
    // v_kernel_mem.sort_unstable_by_key(|m| m.buffer_ptr);
    // log!("v_kernel_mem={v_kernel_mem:#?}");

    let physical_buffer_start = physical_mem().mem as usize;
    let physical_buffer_len = config::total_mem_size();
    let physical_buffer_end = physical_buffer_start + physical_buffer_len;

    for kernel_mem in &v_kernel_mem {
        let KernelMem { alloc_id, size, align, buffer_ptr, .. } = kernel_mem;
        let buffer_ptr = *buffer_ptr as usize;
        if !(physical_buffer_start <= buffer_ptr
            && buffer_ptr + kernel_mem.size as usize <= physical_buffer_end)
        {
            panic!(
                "{kernel_mem:?} doesn't belong to physical_mem {physical_buffer_start:#x}..{physical_buffer_end:#x}"
            );
        }

        // FIXME: why does kernel_mem has provenance?
        // assert!(provenance.is_empty(), "{kernel_mem:?} should not have provenance!");

        ecx.machine.free_alloc_id(
            *alloc_id,
            Size::from_bytes(*size),
            Align::from_bytes(*align).unwrap(),
            MemoryKind::Machine(MiriMemoryKind::Kernel),
        );
        ecx.memory.alloc_map().remove(alloc_id);
    }

    // SAFETY: free the whole physical buffer.
    unsafe {
        std::alloc::dealloc(physical_buffer_start as *mut u8, PhysicalMemory::mem_buffer_layout());
    }
}

/// Returns `(start, len, align)` of the typed object that contains `paddr`, on a
/// page typed as `page_type` with `slot_size`. A typed object is backed by one
/// Miri allocation (created lazily by `lazy_alloc_typed_slot_allocation`).
///
/// - `Slab`: the page holds independent slots laid out from the page start with
///   `step_by(slot_size)`. The slot size need not be a power of two or divide
///   the page size, so the last slot is clipped at the page end: the slab owns
///   only its page. Slot starts are aligned to the largest power of two that
///   divides the slot size.
/// - `PageTable`: the whole page is one object, an array of page-table entries
///   of `slot_size` bytes each. Code indexes entries with pointer arithmetic
///   from the page start (`(page as *mut Entry).add(i)`), which is in bounds
///   only if the object spans the page.
/// - Other kinds: one object per `slot_size` bytes.
pub fn typed_object_span(
    paddr: usize,
    page_type: TypedKind,
    slot_size: usize,
) -> (usize, usize, usize) {
    let page_size = page_size();
    let page_start = paddr - paddr % page_size;
    match page_type {
        TypedKind::Slab => {
            let slot_start = page_start + (paddr - page_start) / slot_size * slot_size;
            let slot_len = slot_size.min(page_start + page_size - slot_start);
            (slot_start, slot_len, 1 << slot_size.trailing_zeros())
        }
        TypedKind::PageTable => (page_start, page_size, page_size),
        _ => (paddr - paddr % slot_size, slot_size, slot_size),
    }
}

/// Frees `count` pages at `paddr` in the simulated physical memory.
pub fn dealloc_pages<'tcx>(
    this: &mut MiriInterpCx<'tcx>,
    paddr: usize,
    count: usize,
) -> InterpResult<'tcx, ()> {
    let mut global_state = this.machine.alloc_addresses.borrow_mut();
    let physical_mem = physical_mem_mut();

    for page_index in 0..count {
        let page_paddr = paddr + page_size() * page_index;
        let page_idx = page_paddr / page_size();
        let page_info = physical_mem.page_states[page_idx];

        if let PageState::Typed { page_type, slot_size } = page_info {
            // Free the allocation of every typed object on the page (see
            // `typed_object_span`).
            let mut actual_paddr = page_paddr;
            while actual_paddr < page_paddr + page_size() {
                let (object_start, object_len, _) =
                    typed_object_span(actual_paddr, page_type, slot_size);
                debug_assert_eq!(object_start, actual_paddr);
                actual_paddr += object_len;
                let pos = global_state
                    .int_to_ptr_map
                    .binary_search_by_key(&(object_start as u64), |(addr, _)| *addr);
                if let Ok(pos) = pos {
                    let dead_id = global_state.int_to_ptr_map[pos].1;
                    physical_mem.free_slab_slots.remove(&dead_id);
                    global_state.int_to_ptr_map.remove(pos);
                    global_state.exposed.remove(&dead_id);
                    global_state.base_paddr.remove(&dead_id);
                    this.memory.alloc_map().remove(&dead_id);
                }
            }
        }
        if physical_mem.page_states[page_idx] == PageState::Unused {
            throw_ub_format!(
                "Page state UB: Attempting to release an unused page. The paddr is 0x{:x}",
                page_paddr
            );
        }
        physical_mem.set_page_state(page_paddr, PageState::Unused);
        physical_mem.remove_init_mask(page_paddr);
    }
    interp_ok(())
}

/// Types `count` pages starting from `paddr` in the simulated physical memory.
pub fn type_pages_at<'tcx>(
    paddr: usize,
    count: usize,
    slot_size: usize,
    page_type: TypedKind,
) -> InterpResult<'tcx, ()> {
    let physical_mem = physical_mem_mut();
    let page_size = page_size();
    let page_state = PageState::Typed { page_type, slot_size };
    for page_index in 0..count {
        let page_paddr = paddr + page_size * page_index;
        physical_mem.set_page_state(page_paddr, page_state);
    }
    // println!(
    //     "[kern_miri_retype_pages] paddr=0x{paddr:x}..0x{:x} => {page_state:?}",
    //     paddr + count * page_size
    // );

    interp_ok(())
}

/// Copies an untyped range `(dst, src, len)` given in virtual addresses.
/// The range is split into page-bounded physical chunks by walking the page
/// table. Every source is snapshotted before writing: virtual mappings can
/// physically overlap. Typed objects must use interpreter memory operations,
/// not this byte-only effect.
pub fn physical_copy<'tcx>(
    mut dst: usize,
    mut src: usize,
    mut len: usize,
) -> InterpResult<'tcx, ()> {
    // The caller hands us virtual addresses, so reject ranges that don't fit in
    // `usize` or the backing memory before the split loop below, where an
    // overflowing `src += chunk_len` could otherwise wrap around forever.
    if dst.checked_add(len).is_none() || src.checked_add(len).is_none() || len > total_mem_size() {
        throw_unsup_format!("untyped physical copy virtual range overflow or too large");
    }

    let page_size = page_size();
    let mem = physical_mem();

    // Walk the virtual range page by page, splitting it into page-bounded
    // chunks (the init-mask shadow is tracked per page, so no chunk may cross
    // a page boundary on either side) and snapshotting each source chunk.
    // Sources are snapshotted before any write because distinct virtual ranges
    // can map onto the same physical page: writing one chunk first would
    // clobber the bytes another chunk has yet to read (memmove-like overlap).
    let mut snapshots = Vec::new();
    while len > 0 {
        // Bytes left until the end of the current page on each side; the
        // smaller one is the longest copy that keeps both sides within a page.
        // Translate each virtual address to physical via the page table,
        // falling back to the identity map when no page table is installed.
        let Some(real_dst) = page_walk_or(dst, || dst) else {
            throw_unsup_format!("untyped physical copy unmapped destination");
        };
        let Some(real_src) = page_walk_or(src, || src) else {
            throw_unsup_format!("untyped physical copy unmapped source");
        };
        // Bytes to copy this iteration: capped by the remaining total and by
        // the page boundary on both sides.
        let chunk_len = {
            let dst_remain = page_size - dst % page_size;
            let src_remain = page_size - src % page_size;
            let remain = core::cmp::min(dst_remain, src_remain);
            core::cmp::min(len, remain)
        };

        for addr in [real_src, real_dst] {
            // Only untyped pages may be copied byte-for-byte; typed pages must
            // go through interpreter memory operations to preserve their tags.
            if mem.page_states.get(addr / page_size) != Some(&PageState::Untyped) {
                throw_unsup_format!("untyped physical copy requires exclusively untyped pages");
            }
            // Every page carries an init-mask allocation that shadows its
            // per-byte initialization state; without it there is nothing to copy.
            let Some(mask) = mem.init_masks.get(&(addr - addr % page_size)) else {
                throw_unsup_format!("untyped physical copy missing initialization shadow");
            };
            // Raw bytes cannot represent pointer provenance, so refuse to copy
            // a page that carries any.
            if mask.provenance().provenances().next().is_some() {
                throw_unsup_format!("untyped physical copy cannot represent pointer provenance");
            }
        }

        // Capture both the raw bytes and their init bitmask: a byte-level copy
        // must move the values *and* which of them are initialized.
        let src_offset = real_src % page_size;
        let source = &mem.init_masks[&(real_src - src_offset)];
        let range = (src_offset..src_offset + chunk_len).into();
        // `.to_vec()` makes this an owned snapshot: `get_bytes_unchecked` returns
        // a slice straight into the physical buffer, and a later write of an
        // earlier chunk must not clobber bytes a later chunk has yet to read.
        let bytes = source.get_bytes_unchecked(range).to_vec();
        let mask = source.init_mask().prepare_copy(range);
        snapshots.push((real_dst, bytes, mask));

        len -= chunk_len;
        src += chunk_len;
        dst += chunk_len;
    }

    // Apply the snapshots. Bounds/state/provenance were checked for every chunk
    // above. Unknown bytes remain unknown; this is a plain byte copy that does
    // not create a new typed allocation or tag.
    let mem = physical_mem_mut();
    for (dst, bytes, mask) in snapshots {
        let dst_offset = dst % page_size;
        let target = mem.init_masks.get_mut(&(dst - dst_offset)).unwrap();
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                target.get_bytes_unchecked_raw_mut().add(dst_offset),
                bytes.len(),
            );
        }
        target.init_mask_apply_copy(mask, (dst_offset..dst_offset + bytes.len()).into(), 1);
    }
    interp_ok(())
}

/// Returns the physical address of the `len` bytes at `vaddr`, after checking
/// that they lie on an untyped page and overlap no interpreter allocation.
/// `len` does not cross a page boundary.
fn untyped_paddr<'tcx>(
    this: &mut MiriInterpCx<'tcx>,
    vaddr: usize,
    len: usize,
    write: bool,
) -> InterpResult<'tcx, usize> {
    let page_size = page_size();
    // The same translation as for any access through a pointer cast from an integer.
    let Some(paddr) = this.kernel_vaddr_to_paddr(vaddr as u64).map(|paddr| paddr as usize) else {
        // Natively the fallible copy would return the number of bytes it could
        // not copy, and the infallible one would fault. Neither is modeled yet.
        throw_unsup_format!("kernel copy touches the unmapped address {vaddr:#x}");
    };
    if paddr % page_size != vaddr % page_size {
        throw_unsup_format!("kernel copy through a mapping that is not page-aligned at {vaddr:#x}");
    }
    if physical_mem().page_states.get(paddr / page_size) != Some(&PageState::Untyped) {
        // Mapped memory that does not hold raw bytes, e.g. a slab page. An
        // invalid access there is reported like any access through a pointer
        // cast from an integer; a valid one is not supported, as the copy
        // reached it from untyped memory.
        let ptr = Pointer::new(Some(Provenance::Wildcard), Size::from_bytes(vaddr as u64));
        let size = Size::from_bytes(len as u64);
        if write {
            this.get_ptr_alloc_mut(ptr, size)?;
        } else {
            this.get_ptr_alloc(ptr, size)?;
        }
        throw_unsup_format!("kernel copy touches {vaddr:#x}, which is not an untyped page");
    }
    // An interpreter allocation placed on untyped pages (a large heap object,
    // a task stack) owns its bytes' initialization state, so the page shadow
    // must not be used for them.
    if let Some(base) = overlapping_allocation(this, paddr, len) {
        throw_unsup_format!(
            "kernel copy range at {vaddr:#x} runs from untyped pages into the interpreter allocation at paddr {base:#x}"
        );
    }
    let Some(mask) = physical_mem().init_masks.get(&(paddr - paddr % page_size)) else {
        throw_unsup_format!("kernel copy missing initialization shadow");
    };
    // Raw bytes cannot represent pointer provenance (as in `physical_copy`).
    if mask.provenance().provenances().next().is_some() {
        throw_unsup_format!("kernel copy cannot represent pointer provenance on untyped pages");
    }
    interp_ok(paddr)
}

/// Returns the physical base address of a live, non-empty interpreter
/// allocation that overlaps the `len` bytes at physical address `paddr`.
fn overlapping_allocation(this: &MiriInterpCx<'_>, paddr: usize, len: usize) -> Option<u64> {
    let (start, end) = (paddr as u64, (paddr + len) as u64);
    let candidates: Vec<(u64, AllocId)> = {
        let global_state = this.machine.alloc_addresses.borrow();
        let map = &global_state.int_to_ptr_map;
        let first = map.partition_point(|(base, _)| *base < start);
        let last = map.partition_point(|(base, _)| *base < end);
        // Allocations do not overlap each other, so besides those starting in
        // the range only the one just below it can reach into it.
        map[first.saturating_sub(1)..last].to_vec()
    };
    candidates
        .into_iter()
        .find(|&(base, alloc_id)| {
            let size = this.get_alloc_info(alloc_id).size.bytes();
            size > 0 && base < end && base + size > start
        })
        .map(|(base, _)| base)
}

/// Copies `len` bytes from `src` to `dst`, as the kernel's `memcpy` (natively
/// `volatile_copy_memory`) does.
///
/// Each side of the copy is either one interpreter allocation (this includes a
/// large heap object or a task stack placed on untyped frames) or untyped
/// pages (raw bytes with a per-byte initialization shadow, e.g. a page cache
/// frame):
///
/// - A pointer with the provenance of an allocation names that allocation, and
///   the whole range is checked like any access through the pointer: a freed
///   object or a range past its end is reported.
/// - A pointer without such provenance (a plain integer address, or a pointer
///   cast from one) names the allocation that an access through a pointer cast
///   from that address would find, checked the same way. If there is none, the
///   side is untyped pages. This is exactly as permissive as such an access:
///   e.g. a freed slab slot is covered by a stand-in allocation, and an address
///   one past the end of an allocation is not attributed to it.
///
/// If both sides are allocations, this is the interpreter's own overlapping
/// copy, which also carries pointer provenance. Otherwise the copy goes in
/// page-bounded chunks, and every chunk of the source is read before any is
/// written, so overlapping ranges behave like `memmove`. Allocations are read
/// and written with the usual checks (aliasing model, data races).
/// Initialization state is copied byte by byte in both directions; pointer
/// provenance is not, as raw bytes cannot hold it. An untyped side that runs
/// into an allocation is not supported.
pub fn kernel_copy<'tcx>(
    this: &mut MiriInterpCx<'tcx>,
    dst: Pointer,
    src: Pointer,
    len: usize,
) -> InterpResult<'tcx, ()> {
    if len == 0 {
        return interp_ok(());
    }
    let (dst_addr, src_addr) = (dst.addr().bytes() as usize, src.addr().bytes() as usize);
    if dst_addr.checked_add(len).is_none() || src_addr.checked_add(len).is_none() {
        throw_unsup_format!("kernel copy virtual range overflow");
    }
    let size = Size::from_bytes(len as u64);

    // Returns the pointer to access the side through if it is an allocation.
    let allocation =
        |this: &MiriInterpCx<'tcx>, ptr: Pointer| -> InterpResult<'tcx, Option<Pointer>> {
            let ptr = if let Some(Provenance::Concrete { .. }) = ptr.provenance {
                ptr
            } else {
                let ptr = Pointer::new(Some(Provenance::Wildcard), ptr.addr());
                if this.ptr_try_get_alloc_id(ptr, len as i64).is_err() {
                    return interp_ok(None);
                }
                ptr
            };
            this.check_ptr_access(ptr, size, CheckInAllocMsg::MemoryAccess)?;
            interp_ok(Some(ptr))
        };
    let src_allocation = allocation(this, src)?;
    let dst_allocation = allocation(this, dst)?;
    if let (Some(src), Some(dst)) = (src_allocation, dst_allocation) {
        return this.mem_copy(src, dst, size, false);
    }
    if len > total_mem_size() {
        throw_unsup_format!("kernel copy larger than physical memory");
    }

    let page_size = page_size();
    let mut snapshots = Vec::new();
    let mut done = 0;
    while done < len {
        let (s, d) = (src_addr + done, dst_addr + done);
        let n = (page_size - s % page_size).min(page_size - d % page_size).min(len - done);
        let chunk = Size::from_bytes(n as u64);
        let snapshot = if let Some(ptr) = src_allocation {
            let ptr = ptr.wrapping_offset(Size::from_bytes(done as u64), this);
            this.get_ptr_alloc(ptr, chunk)?;
            let (alloc_id, offset, _) = this.ptr_get_alloc_id(ptr, n as i64)?;
            let alloc = this.get_alloc_raw(alloc_id)?;
            let range = alloc_range(offset, chunk);
            (alloc.get_bytes_unchecked(range).to_vec(), alloc.init_mask().prepare_copy(range))
        } else {
            let paddr = untyped_paddr(this, s, n, false)?;
            let offset = paddr % page_size;
            let mask = &physical_mem().init_masks[&(paddr - offset)];
            let range = (offset..offset + n).into();
            (mask.get_bytes_unchecked(range).to_vec(), mask.init_mask().prepare_copy(range))
        };
        snapshots.push((done, snapshot));
        done += n;
    }

    for (done, (bytes, init)) in snapshots {
        let n = bytes.len();
        if let Some(ptr) = dst_allocation {
            let ptr = ptr.wrapping_offset(Size::from_bytes(done as u64), this);
            this.write_bytes_ptr(ptr, bytes)?;
            let (alloc_id, offset, _) = this.ptr_get_alloc_id(ptr, n as i64)?;
            let (alloc, _) = this.get_alloc_raw_mut(alloc_id)?;
            alloc.init_mask_apply_copy(init, alloc_range(offset, Size::from_bytes(n as u64)), 1);
        } else {
            let paddr = untyped_paddr(this, dst_addr + done, n, true)?;
            let offset = paddr % page_size;
            let mask = physical_mem_mut().init_masks.get_mut(&(paddr - offset)).unwrap();
            unsafe {
                core::ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    mask.get_bytes_unchecked_raw_mut().add(offset),
                    n,
                );
            }
            mask.init_mask_apply_copy(init, (offset..offset + n).into(), 1);
        }
    }
    interp_ok(())
}

pub fn physical_write_bytes<'tcx>(
    paddr: usize,
    bytes: &[u8],
    ecx: &MiriInterpCx<'_>,
) -> InterpResult<'tcx, ()> {
    // Copy `len` bytes out of the Miri allocation `data` into physical
    // memory at `paddr`, then mark them initialized. Generalizes
    // `kern_miri_zero` to arbitrary byte patterns (still only usable
    // on `Untyped` pages, like the zeroing shim).
    let actual_ptr = mirch::paddr_to_mem(paddr);
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), actual_ptr, bytes.len());
    }
    // Mark the touched bytes initialized in their pages' init-mask
    // shadows; a range may span several pages.
    let page_size = mirch::page_size();
    let mut remaining = bytes.len();
    let mut addr = paddr;
    while remaining > 0 {
        let offset = addr % page_size;
        let chunk = core::cmp::min(remaining, page_size - offset);
        let init_masks = &mut mirch::physical_mem_mut().init_masks;
        let mask_allocation = init_masks.get_mut(&(addr - offset)).unwrap();
        let _ = mask_allocation
            .get_bytes_unchecked_for_overwrite_ptr(ecx, (offset..offset + chunk).into());
        remaining -= chunk;
        addr += chunk;
    }
    interp_ok(())
}

/// Removes the initialization mask for the page at `paddr`.
/// `paddr` is the start of a page.
#[expect(dead_code)]
pub fn remove_init_mask(paddr: usize) {
    let removed = physical_mem_mut().init_masks.remove(&paddr).is_some();
    assert!(removed, "{paddr:#x} has been deallcated, but it's being double freed");
}

/// Checks the page state of the page at `paddr`.
pub fn check_page_state<'tcx>(paddr: usize, page_state: PageState) -> InterpResult<'tcx, ()> {
    let index = paddr / page_size();
    let physical_mem = physical_mem();
    let Some(&current) = physical_mem.page_states.get(index) else {
        throw_ub_format!(
            "Page state UB: current page (paddr=0x{paddr:x}, index={index}) is out of range"
        );
    };
    if current != page_state {
        throw_ub_format!(
            "Page state UB: current page (paddr=0x{paddr:x}, index={index}) state is {current:?}, \
             while the expected should be {page_state:?}"
        );
    }
    interp_ok(())
}

/// Sets the page state of the page at `paddr`.
pub fn set_page_state(paddr: usize, page_state: PageState) {
    let index = paddr / page_size();
    physical_mem_mut().page_states[index] = page_state;
}

/// Sets the root page table.
pub fn set_page_table(page_table: PageTable) {
    // println!("physical_mem has page_table root at paddr=0x{:x}", page_table.root_paddr());
    physical_mem_mut().page_table = Some(page_table);
}

/// Walks the page table to find the physical address corresponding to the given virtual address.
///
///  If the page table is not set, it calls the provided function and return the result.
pub fn page_walk_or<F>(vaddr: usize, func: F) -> Option<usize>
where
    F: FnOnce() -> usize,
{
    if let Some(page_table) = &physical_mem().page_table {
        page_table.page_walk(vaddr)
    } else {
        Some(func())
    }
}

/// Inserts an initialization mask for the page at `paddr`.
/// Currently, only be called in the kern_miri_alloc_pages shim.
pub fn insert_init_mask(this: &MiriInterpCx<'_>, paddr: usize, params: MiriAllocParams) {
    unsafe {
        let layout = Layout::from_size_align_unchecked(page_size(), 1);
        let mut allocation = create_allocation_at(paddr, layout, params);
        allocation.write_uninit(this, (0..page_size()).into());

        physical_mem_mut().init_masks.insert(paddr, allocation);
    }
}

pub struct PhysicalMemory {
    pub mem: *mut u8,
    pub page_states: Vec<PageState>,
    pub init_masks: BTreeMap<usize, Allocation<Provenance, (), MiriAllocBytes>>,
    /// Allocations that stand for free slab slots rather than for heap
    /// objects; see `lazy_alloc_typed_slot_allocation`.
    pub free_slab_slots: BTreeSet<AllocId>,
    /// Object size for the heap slot allocation being created by
    /// `kern_miri_alloc_heap_slot`.
    pub pending_heap_slot_size: Option<usize>,
    pub page_table: Option<PageTable>,
    /// For each physical address of untyped memory that an atomic access
    /// (`kernel_atomic_u32`) has written, the clock its writers released, so
    /// that later atomic accesses there synchronize with them as on hardware.
    pub untyped_atomic_clocks: BTreeMap<u64, VClock>,
}

impl PhysicalMemory {
    pub const fn empty() -> Self {
        Self {
            mem: std::ptr::null_mut(),
            page_states: Vec::new(),
            init_masks: BTreeMap::new(),
            free_slab_slots: BTreeSet::new(),
            pending_heap_slot_size: None,
            page_table: None,
            untyped_atomic_clocks: BTreeMap::new(),
        }
    }

    pub fn mem_buffer_layout() -> Layout {
        Layout::from_size_align(total_mem_size(), page_size()).unwrap()
    }

    pub fn new(config: PhysConfig, page_table_enabled: bool) -> Self {
        super::config::init(config);
        let mem = unsafe { std::alloc::alloc_zeroed(Self::mem_buffer_layout()) };

        let page_states =
            if page_table_enabled { vec![PageState::Unused; total_page_num()] } else { vec![] };

        Self {
            mem,
            page_states,
            init_masks: BTreeMap::new(),
            free_slab_slots: BTreeSet::new(),
            pending_heap_slot_size: None,
            page_table: None,
            untyped_atomic_clocks: BTreeMap::new(),
        }
    }
}

impl PhysicalMemory {
    pub fn remove_init_mask(&mut self, paddr: usize) {
        self.init_masks.remove(&paddr);
    }

    #[expect(unused)]
    pub fn page_state_matches(&self, paddr: usize, page_state: PageState) -> InterpResult<'_, ()> {
        let index = paddr / page_size();
        let Some(&current) = self.page_states.get(index) else {
            throw_ub_format!(
                "Page state UB: current page (paddr=0x{paddr:x}, index={index}) is out of range"
            );
        };
        if current != page_state {
            throw_ub_format!("Page state UB: current page state is {current:?}");
        }
        interp_ok(())
    }

    pub fn set_page_state(&mut self, paddr: usize, page_state: PageState) {
        let index = paddr / page_size();
        self.page_states[index] = page_state;
    }
}

/// Additional state settings for the physical pages maintained by Miri.
/// Initially, all pages are set to `Unused`.
/// PageState transformation:
/// `Unused` --allocate--> `Untyped` --retype--> `Typed`.
/// `Untyped`/`Typed` --deallocate--> `Unused`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PageState {
    Unused,
    Untyped,
    /// Page type means what the page is used for.
    /// Slot size means the slot element is the page has the size and alignment.
    Typed {
        page_type: TypedKind,
        slot_size: usize,
    },
}

#[repr(usize)]
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TypedKind {
    Slab = 1,
    PageTable = 2,
    Stack = 3,
    Interpreter = 4,
    /// A copy of the CPU-local section, made by `kern_miri_copy_cpu_local_section`.
    CpuLocalCopy = 5,
}

impl TypedKind {
    pub fn from_usize(value: usize) -> Option<Self> {
        match value {
            1 => Some(TypedKind::Slab),
            2 => Some(TypedKind::PageTable),
            3 => Some(TypedKind::Stack),
            4 => Some(TypedKind::Interpreter),
            _ => None,
        }
    }
}

/// Whether an access of `len` bytes through `ptr` lands on untyped pages: the
/// pointer has no allocation's provenance, an access through a pointer cast
/// from its address would find no allocation, and the address maps to an
/// untyped page.
fn on_untyped_page<'tcx>(this: &MiriInterpCx<'tcx>, ptr: Pointer, len: usize) -> bool {
    if matches!(ptr.provenance, Some(Provenance::Concrete { .. }))
        || len == 0
        || physical_mem().page_states.is_empty()
        || this.machine.alloc_addresses.borrow().provenance_mode == ProvenanceMode::Strict
    {
        // With strict provenance an address cannot name memory at all.
        return false;
    }
    let wildcard = Pointer::new(Some(Provenance::Wildcard), ptr.addr());
    if this.ptr_try_get_alloc_id(wildcard, len as i64).is_ok() {
        return false;
    }
    let Some(paddr) = this.kernel_vaddr_to_paddr(ptr.addr().bytes()) else {
        return false;
    };
    physical_mem().page_states.get(paddr as usize / page_size()) == Some(&PageState::Untyped)
}

/// Performs the volatile intrinsic `instance` if its pointer lands on untyped
/// pages (see `on_untyped_page`), and returns whether it did. Such pages hold
/// raw bytes, not an interpreter allocation, so the access goes through
/// `kernel_copy` with a temporary allocation on the other side: the
/// initialization state moves byte by byte, pointer provenance does not (raw
/// bytes cannot hold it), and a range that leaves the untyped pages is reported
/// by `kernel_copy`. Reading uninitialized bytes into a typed value is then
/// reported when the value is copied out, as for any other load.
pub fn volatile_on_untyped<'tcx>(
    this: &mut MiriInterpCx<'tcx>,
    instance: ty::Instance<'tcx>,
    args: &[OpTy<'tcx>],
    dest: &PlaceTy<'tcx>,
) -> InterpResult<'tcx, bool> {
    let kind = MiriMemoryKind::Machine.into();
    let name = this.tcx.item_name(instance.def_id());
    match name.as_str() {
        name @ ("volatile_load" | "unaligned_volatile_load") => {
            let [ptr] = args else { return interp_ok(false) };
            let ptr = this.read_pointer(ptr)?;
            if !on_untyped_page(this, ptr, dest.layout.size.bytes_usize()) {
                return interp_ok(false);
            }
            if name == "volatile_load" {
                this.check_ptr_align(ptr, dest.layout.align)?;
            }
            let tmp = this.allocate(dest.layout, kind)?;
            kernel_copy(this, tmp.ptr(), ptr, dest.layout.size.bytes_usize())?;
            this.copy_op(&tmp, dest)?;
            this.deallocate_ptr(tmp.ptr(), None, kind)?;
        }
        name @ ("volatile_store" | "unaligned_volatile_store") => {
            let [ptr, val] = args else { return interp_ok(false) };
            let ptr = this.read_pointer(ptr)?;
            if !on_untyped_page(this, ptr, val.layout.size.bytes_usize()) {
                return interp_ok(false);
            }
            if name == "volatile_store" {
                this.check_ptr_align(ptr, val.layout.align)?;
            }
            let tmp = this.allocate(val.layout, kind)?;
            this.copy_op(val, &tmp)?;
            kernel_copy(this, ptr, tmp.ptr(), val.layout.size.bytes_usize())?;
            this.deallocate_ptr(tmp.ptr(), None, kind)?;
        }
        "volatile_set_memory" => {
            let [ptr, byte, count] = args else { return interp_ok(false) };
            let layout = this.layout_of(instance.args.type_at(0))?;
            let ptr = this.read_pointer(ptr)?;
            let byte = this.read_scalar(byte)?.to_u8()?;
            let count = this.read_target_usize(count)?;
            let Some(len) = layout.size.bytes().checked_mul(count) else {
                throw_ub_format!("overflow computing total size of `volatile_set_memory`");
            };
            let len = usize::try_from(len).unwrap();
            if !on_untyped_page(this, ptr, len) {
                return interp_ok(false);
            }
            this.check_ptr_align(ptr, layout.align)?;
            // Bound the temporary buffer before allocating it (`kernel_copy`
            // would reject the length only afterwards).
            if len > total_mem_size() {
                throw_unsup_format!("volatile_set_memory larger than physical memory");
            }
            let size = Size::from_bytes(len as u64);
            let tmp: Pointer = this.allocate_ptr(size, Align::ONE, kind, AllocInit::Uninit)?.into();
            this.write_bytes_ptr(tmp, std::iter::repeat_n(byte, len))?;
            kernel_copy(this, ptr, tmp, len)?;
            this.deallocate_ptr(tmp, None, kind)?;
        }
        _ => return interp_ok(false),
    }
    interp_ok(true)
}

/// Performs a sequentially consistent atomic load (`cmpxchg` is `None`) or
/// compare-exchange (`Some((expected, new))`) of the `u32` at `ptr`, as one
/// step of the machine, and returns the value read.
///
/// If `ptr` names an interpreter allocation (by its provenance, or because an
/// access through a pointer cast from its address would find one), this is an
/// ordinary atomic access, with Miri's checks for it (bounds, alignment, data
/// races, and the aliasing model as far as the pointer's provenance allows).
/// Otherwise, if the four bytes lie on untyped pages, they are read (and
/// possibly written) through `kernel_copy`, like a volatile access there: the
/// value must be initialized, and no data race is checked on raw bytes, as for
/// any other access to untyped pages. The access still synchronizes like a
/// sequentially consistent one: it acquires what earlier successful
/// compare-exchanges of that physical address released, and a successful
/// compare-exchange releases into it. Clocks are kept when a page is freed, so a
/// reused page may synchronize more than on hardware; that can hide a data race
/// but never report one that is not there.
pub fn kernel_atomic_u32<'tcx>(
    this: &mut MiriInterpCx<'tcx>,
    ptr: Pointer,
    cmpxchg: Option<(u32, u32)>,
) -> InterpResult<'tcx, u32> {
    let layout = this.machine.layouts.u32;
    if on_untyped_page(this, ptr, 4) {
        this.check_ptr_align(ptr, layout.align)?;
        let paddr = this.kernel_vaddr_to_paddr(ptr.addr().bytes()).unwrap();
        if let Some(clock) = physical_mem().untyped_atomic_clocks.get(&paddr).cloned() {
            this.acquire_clock(&clock)?;
        }
        let kind = MiriMemoryKind::Machine.into();
        let tmp = this.allocate(layout, kind)?;
        kernel_copy(this, tmp.ptr(), ptr, 4)?;
        let current = this.read_scalar(&tmp)?.to_u32()?;
        if let Some((expected, new)) = cmpxchg
            && current == expected
        {
            this.write_scalar(Scalar::from_u32(new), &tmp)?;
            kernel_copy(this, ptr, tmp.ptr(), 4)?;
            // A read-modify-write continues the release sequence: join, not replace.
            this.release_clock(|clock| {
                physical_mem_mut().untyped_atomic_clocks.entry(paddr).or_default().join(clock)
            })?;
        }
        this.deallocate_ptr(tmp.ptr(), None, kind)?;
        return interp_ok(current);
    }
    let ptr = if let Some(Provenance::Concrete { .. }) = ptr.provenance {
        ptr
    } else {
        Pointer::new(Some(Provenance::Wildcard), ptr.addr())
    };
    let place = this.ptr_to_mplace(ptr, layout);
    let current = match cmpxchg {
        None => this.read_scalar_atomic(&place, AtomicReadOrd::SeqCst)?,
        Some((expected, new)) => {
            let expected = ImmTy::from_scalar(Scalar::from_u32(expected), layout);
            this.atomic_compare_exchange(
                &place,
                &expected,
                Scalar::from_u32(new),
                AtomicRwOrd::SeqCst,
                AtomicReadOrd::SeqCst,
                false,
            )?
            .0
        }
    };
    interp_ok(current.to_u32()?)
}

/// Whether the kernel virtual address `vaddr` maps, through the current page
/// table, to a physical address outside RAM. KMiri models no device there.
pub fn maps_outside_ram(vaddr: usize) -> bool {
    let Some(page_table) = &physical_mem().page_table else {
        return false;
    };
    page_table.page_walk(vaddr).is_some_and(|paddr| paddr >= total_mem_size())
}
