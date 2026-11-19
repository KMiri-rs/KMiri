use std::alloc::Layout;
use std::collections::BTreeMap;

use rustc_abi::{Align, Size};
use rustc_middle::ty::Mutability;

use super::PageTable;
use super::config::*;
use crate::alloc::MiriAllocParams;
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

        if let PageState::Typed { page_type: _, slot_size } = page_info {
            for index in 0..page_size() / slot_size {
                let actual_paddr = page_paddr + index * slot_size;
                let pos = global_state
                    .int_to_ptr_map
                    .binary_search_by_key(&(actual_paddr as u64), |(addr, _)| *addr);
                if let Ok(pos) = pos {
                    let dead_id = global_state.int_to_ptr_map[pos].1;
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
    pub heap_slot_next: BTreeMap<usize, usize>,
    /// Object size for the heap slot allocation being created by
    /// `kern_miri_alloc_heap_slot`.
    pub pending_heap_slot_size: Option<usize>,
    pub page_table: Option<PageTable>,
}

impl PhysicalMemory {
    pub const fn empty() -> Self {
        Self {
            mem: std::ptr::null_mut(),
            page_states: Vec::new(),
            init_masks: BTreeMap::new(),
            heap_slot_next: BTreeMap::new(),
            pending_heap_slot_size: None,
            page_table: None,
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
            heap_slot_next: BTreeMap::new(),
            pending_heap_slot_size: None,
            page_table: None,
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
