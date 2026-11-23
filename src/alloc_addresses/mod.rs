//! This module is responsible for managing the absolute addresses that allocations are located at,
//! and for casting between pointers and integers based on those addresses.

mod address_generator;
mod reuse_pool;

use std::alloc::Layout;
use std::cell::RefCell;

use rand::RngExt;
use rustc_abi::{Align, Size};
use rustc_data_structures::fx::{FxHashMap, FxHashSet};
use rustc_middle::ty::TyCtxt;

pub use self::address_generator::AddressGenerator;
use self::reuse_pool::ReusePool;
use crate::alloc::MiriAllocParams;
use crate::alloc_addresses::address_generator::align_addr;
use crate::concurrency::{AllocDataRaceHandler, GlobalDataRaceHandler, VClock, data_race};
use crate::diagnostics::SpanDedupDiagnostic;
use crate::helpers::adjust_stack_addr;
use crate::mirch::{CodeSection, PageState, TypedKind, kernel_code_paddr_to_vaddr};
use crate::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ProvenanceMode {
    /// We support `expose_provenance`/`with_exposed_provenance` via "wildcard" provenance.
    /// However, we warn on `with_exposed_provenance` to alert the user of the precision loss.
    Default,
    /// Like `Default`, but without the warning.
    Permissive,
    /// We error on `with_exposed_provenance`, ensuring no precision loss.
    Strict,
}

pub type GlobalState = RefCell<GlobalStateInner>;

/// A copy of the CPU-local section that the program made at physical address `base`.
/// Each `.cpu_local` variable has its own allocation in the copy, at the variable's
/// offset in the section.
#[derive(Debug)]
pub struct CpuLocalCopy {
    base: usize,
    /// The variables that had been used when the copy was made. Their copies were
    /// made then, with their values at that time.
    copied: FxHashSet<AllocId>,
    /// When the copy was made. A variable that had not been used by then still held
    /// its initial value; its copy is created when it is first accessed, holding that
    /// value, as if written at this point.
    made_at: Option<data_race::MadeAt>,
}

#[derive(Debug)]
pub struct GlobalStateInner {
    /// This is used as a map between the address of each allocation and its `AllocId`. It is always
    /// sorted by address. We cannot use a `HashMap` since we can be given an address that is offset
    /// from the base address, and we need to find the `AllocId` it belongs to. This is not the
    /// *full* inverse of `base_addr`; dead allocations have been removed.
    /// Note that in GenMC mode, dead allocations are *not* removed -- and also, addresses are never
    /// reused. This lets us use the address as a cross-execution-stable identifier for an allocation.
    /// kmiri: the u64 is a paddr.
    pub int_to_ptr_map: Vec<(u64, AllocId)>,
    /// The base address for each allocation.  We cannot put that into
    /// `AllocExtra` because function pointers also have a base address, and
    /// they do not have an `AllocExtra`.
    /// This is the inverse of `int_to_ptr_map`.
    /// NOTE: the ptr in int_to_ptr_map and base_paddr are physical address.
    pub base_paddr: FxHashMap<AllocId, u64>,

    /// kmimri: the key is paddr, the value is vaddr.
    /// This is a workaround for now to hot fix the non-linear mapping.
    paddr_to_vaddr: FxHashMap<u64, u64>,

    /// The set of exposed allocations. This cannot be put
    /// into `AllocExtra` for the same reason as `base_addr`.
    pub exposed: FxHashSet<AllocId>,
    /// The provenance to use for int2ptr casts
    pub provenance_mode: ProvenanceMode,
    /// The generator for new addresses in a given range, and a pool for address reuse. This is
    /// `None` if addresses are generated elsewhere (in native-lib mode or with GenMC).
    address_generation: Option<(AddressGenerator, ReusePool)>,
    /// Native-lib mode only: Temporarily store prepared memory space for global allocations the
    /// first time their memory address is required. This is used to ensure that the memory is
    /// allocated before Miri assigns it an internal address, which is important for matching the
    /// internal address to the machine address so FFI can read from pointers.
    prepared_alloc_bytes: Option<FxHashMap<AllocId, MiriAllocBytes>>,

    /// This is used as a memory address when a new pointer is casted to an integer. It
    /// is always larger than any address that was previously made part of a block.
    /// This is used for allocating addresses for non stack and non cpu-local allocations.
    next_base_vaddr: u64,
    /// This is used for allocating addresses for cpu-local allocations.
    next_cpu_local_paddr: u64,
    /// This is used for allocating addresses for stack allocations.
    #[expect(unused)]
    next_stack_vaddr: u64,
    /// Next virtual address cursor for non-global allocations.
    next_dyn_vaddr: u64,
    /// kmiri: `extern` functions whose address the kernel config gives in `[layout]`.
    /// The address is the symbol value. A function holds no memory, so these stay out of
    /// `base_paddr` and `int_to_ptr_map`, where a data allocation may sit at the same address.
    layout_fn_vaddr: FxHashMap<AllocId, u64>,
}

impl VisitProvenance for GlobalStateInner {
    fn visit_provenance(&self, _visit: &mut VisitWith<'_>) {
        let GlobalStateInner {
            int_to_ptr_map: _,
            base_paddr: _,
            paddr_to_vaddr: _,
            prepared_alloc_bytes: _,
            exposed: _,
            address_generation: _,
            provenance_mode: _,
            next_base_vaddr: _,
            next_cpu_local_paddr: _,
            next_stack_vaddr: _,
            next_dyn_vaddr: _,
            layout_fn_vaddr: _,
        } = self;
        // Though base_addr, int_to_ptr_map, and exposed contain AllocIds, we do not want to visit them.
        // int_to_ptr_map and exposed must contain only live allocations, and those
        // are never garbage collected.
        // base_addr is only relevant if we have a pointer to an AllocId and need to look up its
        // base address; so if an AllocId is not reachable from somewhere else we can remove it
        // here.
    }
}

impl GlobalStateInner {
    pub fn new<'tcx>(config: &MiriConfig, stack_addr: u64, tcx: TyCtxt<'tcx>) -> Self {
        GlobalStateInner {
            int_to_ptr_map: Vec::default(),
            base_paddr: FxHashMap::default(),
            paddr_to_vaddr: FxHashMap::default(),
            exposed: FxHashSet::default(),
            provenance_mode: config.provenance_mode,
            address_generation: (config.native_lib.is_empty() && config.genmc_config.is_none())
                .then(|| {
                    (
                        AddressGenerator::new(stack_addr..tcx.target_usize_max()),
                        ReusePool::new(config),
                    )
                }),
            prepared_alloc_bytes: (!config.native_lib.is_empty()).then(FxHashMap::default),
            next_base_vaddr: kernel_code_paddr_to_vaddr(mirch::kernel_static_start_addr()) as u64,
            next_stack_vaddr: kernel_code_paddr_to_vaddr(mirch::kernel_stack_end_addr()) as u64,
            next_cpu_local_paddr: kernel_code_paddr_to_vaddr(mirch::cpu_local_start_addr()) as u64,
            next_dyn_vaddr: kernel_code_paddr_to_vaddr(mirch::total_mem_size()) as u64,
            layout_fn_vaddr: FxHashMap::default(),
        }
    }

    /// Returns the miminal stack variable physical addr that is guaranteed to be allocated.
    /// NOTE: the real addr range of the stack variable is `[addr, addr + bytesize)`
    /// where addr is the returned u64, bytesize can be queried through AllocId.
    pub fn min_allocated_stack_paddr(&self) -> Option<(u64, AllocId)> {
        let mut min_allocated_stack_addr: Option<(u64, AllocId)> = None;
        for &(paddr, alloc_id) in &self.int_to_ptr_map {
            if CodeSection::paddr(paddr) == Some(CodeSection::Stack) {
                if let Some((addr, _)) = min_allocated_stack_addr
                    && addr < paddr
                {
                    // The old stack addr has been minimal, thus do nothing.
                    continue;
                }
                min_allocated_stack_addr = Some((paddr, alloc_id));
            }
        }
        min_allocated_stack_addr
    }

    pub fn remove_unreachable_allocs(&mut self, allocs: &LiveAllocs<'_, '_>) {
        // `exposed` and `int_to_ptr_map` are cleared immediately when an allocation
        // is freed, so `base_addr` is the only one we have to clean up based on the GC.
        self.base_paddr.retain(|id, _| allocs.is_live(*id));
    }

    /// Add the info of exposed paddr and AllocId to the interpreter, like int_to_ptr_map.
    fn set_exposed_kernel_padd(&mut self, alloc_id: AllocId, paddr: usize) {
        let paddr = paddr as u64;
        let pos = if self.int_to_ptr_map.last().is_some_and(|(last_addr, _)| *last_addr < paddr) {
            self.int_to_ptr_map.len()
        } else {
            self.int_to_ptr_map.binary_search_by_key(&paddr, |(addr, _)| *addr).unwrap_err()
        };

        self.exposed.insert(alloc_id);
        self.int_to_ptr_map.insert(pos, (paddr, alloc_id));
        self.base_paddr.insert(alloc_id, paddr);
    }

    pub fn get_base_addr(&self, alloc_id: AllocId) -> u64 {
        *self.base_paddr.get(&alloc_id).unwrap()
    }
}

impl<'tcx> EvalContextExtPriv<'tcx> for crate::MiriInterpCx<'tcx> {}
trait EvalContextExtPriv<'tcx>: crate::MiriInterpCxExt<'tcx> {
    // kmiri: u64 returned is a vaddr.
    fn addr_from_alloc_id_uncached(
        &self,
        global_state: &mut GlobalStateInner,
        alloc_id: AllocId,
        memory_kind: MemoryKind,
    ) -> InterpResult<'tcx, u64> {
        let ecx = self.eval_context_ref();
        let info = ecx.get_alloc_info(alloc_id);

        // This is either called immediately after allocation (and then cached), or when
        // adjusting `tcx` pointers (which never get freed). So assert that we are looking
        // at a live allocation. This also ensures that we never re-assign an address to an
        // allocation that previously had an address, but then was freed and the address
        // information was removed.
        assert!(!matches!(info.kind, AllocKind::Dead));

        // TypeId allocations always have a "base address" of 0 (i.e., the relative offset is the
        // hash fragment and therefore equal to the actual integer value).
        if matches!(info.kind, AllocKind::TypeId) {
            return interp_ok(0);
        }

        // Miri's address assignment leaks state across thread boundaries, which is incompatible
        // with GenMC execution. So we instead let GenMC assign addresses to allocations.
        if let Some(genmc_ctx) = ecx.machine.data_race.as_genmc_ref() {
            let addr = genmc_ctx.handle_alloc(ecx, alloc_id, info.size, info.align, memory_kind)?;
            return interp_ok(addr);
        }

        // This allocation does not have a base address yet, pick or reuse one.
        if !ecx.machine.native_lib.is_empty() {
            // In native lib mode, we use the "real" address of the bytes for this allocation.
            // This ensures the interpreted program and native code have the same view of memory.
            let params = ecx.machine.get_default_alloc_params();
            let base_ptr = match info.kind {
                AllocKind::LiveData => {
                    if memory_kind == MiriMemoryKind::Global.into() {
                        // For new global allocations, we always pre-allocate the memory to be able use the machine address directly.
                        let prepared_bytes = MiriAllocBytes::zeroed(info.size, info.align, params)
                            .unwrap_or_else(|| {
                                panic!("Miri ran out of memory: cannot create allocation of {size:?} bytes", size = info.size)
                            });
                        let ptr = prepared_bytes.as_ptr();
                        // Store prepared allocation to be picked up for use later.
                        global_state
                            .prepared_alloc_bytes
                            .as_mut()
                            .unwrap()
                            .try_insert(alloc_id, prepared_bytes)
                            .unwrap();
                        ptr
                    } else {
                        // Non-global allocations are already in memory at this point so
                        // we can just get a pointer to where their data is stored.
                        ecx.get_alloc_raw(alloc_id)?.get_bytes_unchecked_raw()
                    }
                }
                #[cfg(all(feature = "native-lib", unix))]
                AllocKind::Function => {
                    if let Some(GlobalAlloc::Function { instance, .. }) =
                        ecx.tcx.try_get_global_alloc(alloc_id)
                    {
                        let fn_sig = ecx.tcx.instantiate_bound_regions_with_erased(
                            ecx.tcx
                                .fn_sig(instance.def_id())
                                .instantiate(*ecx.tcx, instance.args)
                                .skip_norm_wip(),
                        );
                        let fn_ptr = crate::shims::native_lib::build_libffi_closure(ecx, fn_sig)?;

                        #[expect(
                            clippy::as_conversions,
                            reason = "No better way to cast a function ptr to a ptr"
                        )]
                        {
                            fn_ptr as *const _
                        }
                    } else {
                        dummy_alloc(params)
                    }
                }
                #[cfg(not(all(feature = "native-lib", unix)))]
                AllocKind::Function => dummy_alloc(params),
                AllocKind::VTable | AllocKind::VaList => dummy_alloc(params),
                AllocKind::TypeId | AllocKind::Dead => unreachable!(),
            };
            // We don't have to expose this pointer yet, we do that in `prepare_for_native_call`.
            return interp_ok(base_ptr.addr().to_u64());
        }
        // We are not in native lib or genmc mode, so we control the addresses ourselves.
        let (_addr_gen, reuse) = global_state.address_generation.as_mut().unwrap();
        let mut rng = ecx.machine.rng.borrow_mut();
        // FIXME: need to decide if reusing pointer makes sense in kernel code. At least,
        // it's not meaningful to reuse stack pointer, because it messes up the stack across calls or threads.
        // Disable pointer reuse for now.
        if false
            && let Some((reuse_paddr, clock)) =
                reuse.take_addr(&mut *rng, info.size, info.align, memory_kind, ecx.active_thread())
        {
            // If we use some other thread's address, that implies a happens-before.
            if let Some(clock) = clock {
                ecx.acquire_clock(&clock)?;
            }
            interp_ok(kernel_code_paddr_to_vaddr(reuse_paddr as usize) as u64)
        } else {
            let base_vaddr = if memory_kind == MemoryKind::Stack {
                let thread = ecx.machine.threads.active_thread_ref();
                let mut next_stack_vaddr = thread.next_stack_vaddr.borrow_mut();

                let base_vaddr =
                    adjust_stack_addr(info.size.bytes(), info.align.bytes(), *next_stack_vaddr);

                if base_vaddr < thread.stack_bottom {
                    log!(
                        "[addr_from_alloc_id_uncached - error - stack] `base_vaddr={base_vaddr:#x} < stack_bottom={:#x}` makes AddressSpaceFull",
                        thread.stack_bottom
                    );
                    throw_exhaust!(AddressSpaceFull);
                }
                *next_stack_vaddr = base_vaddr;

                base_vaddr
            } else if ecx.machine.cpu_local_alloc_set.borrow().contains(&alloc_id)
                || memory_kind == MiriMemoryKind::Global.into()
            {
                let (next_vaddr, limit) =
                    if ecx.machine.cpu_local_alloc_set.borrow().contains(&alloc_id) {
                        (
                            &mut global_state.next_cpu_local_paddr,
                            kernel_code_paddr_to_vaddr(mirch::cpu_local_end_addr()) as u64,
                        )
                    } else {
                        (
                            &mut global_state.next_base_vaddr,
                            kernel_code_paddr_to_vaddr(mirch::kernel_static_end_addr()) as u64,
                        )
                    };

                // We have to pick a fresh address.
                // Leave some space to the previous allocation, to give it some chance to be less aligned.
                // We ensure that `(global_state.next_base_addr + slack) % 16` is uniformly distributed.
                let slack = rng.random_range(0..16);
                // From next_base_addr + slack, round up to adjust for alignment.
                let base_vaddr =
                    next_vaddr.checked_add(slack).ok_or_else(|| err_exhaust!(AddressSpaceFull))?;
                let base_vaddr = align_addr(base_vaddr, info.align.bytes());
                if ecx.machine.cpu_local_alloc_set.borrow().contains(&alloc_id) {
                    let end = base_vaddr
                        .checked_add(info.size.bytes().max(1))
                        .ok_or_else(|| err_exhaust!(AddressSpaceFull))?;
                    if end > limit {
                        let start = kernel_code_paddr_to_vaddr(mirch::cpu_local_start_addr());
                        let needed =
                            (end - start as u64).next_multiple_of(mirch::page_size() as u64);
                        throw_unsup_format!(
                            "the CPU-local section is full: a variable of size {:#x} and alignment {:#x} needs a section of at least {needed:#x} bytes, but `cpu_local_size` is {:#x}; increase `cpu_local_size` in the KMiri config",
                            info.size.bytes(),
                            info.align.bytes(),
                            mirch::cpu_local_segment_size(),
                        );
                    }
                }
                if base_vaddr >= limit {
                    log!(
                        "[addr_from_alloc_id_uncached - error] `base_addr={base_vaddr:#x} >= limit={limit:#x}` makes AddressSpaceFull",
                    );
                    throw_exhaust!(AddressSpaceFull);
                }

                // Remember next base address.  If this allocation is zero-sized, leave a gap of at
                // least 1 to avoid two allocations having the same base address. (The logic in
                // `alloc_id_from_addr` assumes unique addresses, and different function/vtable pointers
                // need to be distinguishable!)
                *next_vaddr = base_vaddr
                    .checked_add(info.size.bytes().max(1))
                    .ok_or_else(|| err_exhaust!(AddressSpaceFull))?;
                // Even if `Size` didn't overflow, we might still have filled up the address space.
                if *next_vaddr > ecx.target_usize_max() {
                    throw_exhaust!(AddressSpaceFull);
                }
                base_vaddr
            } else {
                // FIXME: Need a extensible way to support kernel and user code.
                // This case is for user code to put heap allocations.
                // Kernel code usually shouldn't fall into this, and we haven't avoid these pages
                // from collapsing with this allocation.
                // Non-global allocations (e.g. Rust/C heap) use free physical pages.
                // Allocate from the top down to avoid colliding with thread stacks, which grow
                // upward from `kernel_stack_end_addr()`.
                let size = info.size.bytes().max(1);
                let slack = rng.random_range(0..16);

                let end_vaddr = global_state
                    .next_dyn_vaddr
                    .checked_sub(slack)
                    .ok_or_else(|| err_exhaust!(AddressSpaceFull))?;
                let unaligned_base =
                    end_vaddr.checked_sub(size).ok_or_else(|| err_exhaust!(AddressSpaceFull))?;
                let base_vaddr = unaligned_base - (unaligned_base % info.align.bytes());

                let lower = kernel_code_paddr_to_vaddr(mirch::kernel_stack_end_addr()) as u64;
                if base_vaddr < lower {
                    log!(
                        "[addr_from_alloc_id_uncached - error] `base_addr={base_vaddr:#x} < lower={lower:#x}` makes AddressSpaceFull",
                    );
                    throw_exhaust!(AddressSpaceFull);
                }

                global_state.next_dyn_vaddr = base_vaddr;
                base_vaddr
            };

            interp_ok(base_vaddr)
        }
    }
}

fn dummy_alloc(params: MiriAllocParams) -> *const u8 {
    // Allocate some dummy memory to get a unique address for this function/vtable.
    let alloc_bytes = MiriAllocBytes::from_bytes(&[0u8; 1], Align::from_bytes(1).unwrap(), params);
    let ptr = alloc_bytes.as_ptr();
    // Leak the underlying memory to ensure it remains unique.
    std::mem::forget(alloc_bytes);
    ptr
}

impl<'tcx> EvalContextExt<'tcx> for crate::MiriInterpCx<'tcx> {}
pub trait EvalContextExt<'tcx>: crate::MiriInterpCxExt<'tcx> {
    /// Allocates a new allocation at the given address for typed slot.
    ///
    /// If the `paddr` is not referred to a typed slot, it returns `None`.
    fn lazy_alloc_typed_slot_allocation(&self, paddr: usize) -> Option<AllocId> {
        let page_index = paddr / mirch::page_size();
        let page_info = *mirch::physical_mem().page_states.get(page_index)?;

        if let PageState::Typed { page_type, slot_size } = page_info {
            if page_type == TypedKind::CpuLocalCopy {
                // Only `cpu_local_copy_at` creates allocations in a copy of the section.
                return None;
            }
            let (slot_start, slot_len, align) =
                mirch::typed_object_span(paddr, page_type, slot_size);
            // The address may lie in a slot past the end of the live object
            // there, e.g. one past the end of a `Vec` buffer that is smaller
            // than its slot. Such an address is in no allocation (Miri does not
            // recognize an allocation at its end address either), so do not
            // create a second allocation at the slot start.
            let occupied = self
                .eval_context_ref()
                .machine
                .alloc_addresses
                .borrow()
                .int_to_ptr_map
                .binary_search_by_key(&(slot_start as u64), |(addr, _)| *addr)
                .is_ok();
            if occupied {
                return None;
            }
            // A heap object gets an allocation of exactly its own size, like a
            // Rust heap allocation in Miri; other typed slots span the slot.
            if let Some(size) = mirch::physical_mem_mut().pending_heap_slot_size.take() {
                return Some(self.kernel_allocation_at(slot_start, size, align));
            }
            // The slot has existed since the page was typed, however late it is
            // first touched.
            let made_at = self
                .eval_context_ref()
                .machine
                .typed_page_made_at
                .borrow()
                .get(&page_index)
                .copied();
            let alloc_id = self.kernel_allocation_made_at(slot_start, slot_len, align, made_at);
            if page_type == TypedKind::Slab {
                // Without a pending heap object this is the allocator itself
                // accessing a free slot through its address, e.g. the
                // free-list link that `SlabSlotList::push` writes into the
                // slot. The allocation stands for the free slot until
                // `alloc_heap_slot` hands the slot out.
                mirch::physical_mem_mut().free_slab_slots.insert(alloc_id);
            }
            return Some(alloc_id);
        }

        None
    }

    fn dealloc_heap_slot(&mut self, ptr: Pointer, size: usize) -> InterpResult<'tcx, ()> {
        let this = self.eval_context_ref();
        let (alloc_id, _, _) = this.ptr_get_alloc_id(ptr, 0)?;
        if this.memory.alloc_map().get(alloc_id).is_none() {
            return interp_ok(());
        }

        // Deallocate through the pointer the kernel passed to
        // `GlobalAlloc::dealloc`, as Miri's own `__rust_dealloc` shim does.
        // The free-list link is written into the slot only after this, through
        // the slot's address (see `SlabSlotList::push`), so this pointer is
        // still valid.
        // Using the root tag instead would make the deallocation's write
        // access pop the item of the very `Box` being dropped, which is
        // weakly protected while it is passed by value (e.g. to `mem::drop`),
        // and report UB for every such drop.
        let align = this.get_alloc_info(alloc_id).align;
        let kind = MemoryKind::Machine(MiriMemoryKind::Kernel);
        self.eval_context_mut().deallocate_ptr(
            ptr,
            Some((Size::from_bytes(size as u64), align)),
            kind,
        )?;
        interp_ok(())
    }

    /// Creates the allocation for a heap object that the kernel placed
    /// directly on frames from its frame allocator instead of in a slab slot.
    ///
    /// Such frames are untyped pages, so there is no slot to look up: the
    /// object gets an allocation of exactly `size` bytes starting at its first
    /// page. Returns `false` if `vaddr` is not on an untyped page.
    fn alloc_large_heap_object(&self, vaddr: u64, size: usize) -> InterpResult<'tcx, bool> {
        let this = self.eval_context_ref();
        if !this.machine.is_page_table_enabled() {
            return interp_ok(false);
        }
        let page_size = mirch::page_size();
        let vaddr = vaddr as usize;
        let paddr = mirch::page_walk_or(vaddr, || unreachable!())
            .or_else(|| mirch::try_kernel_code_vaddr_to_paddr(vaddr))
            .or_else(|| mirch::try_boot_pt_vaddr_to_paddr(vaddr));
        let Some(paddr) = paddr else {
            return interp_ok(false);
        };
        let is_untyped = |paddr: usize| {
            mirch::physical_mem().page_states.get(paddr / page_size) == Some(&PageState::Untyped)
        };
        if !is_untyped(paddr) {
            return interp_ok(false);
        }

        if paddr % page_size != 0 {
            throw_unsup_format!(
                "KMiri: large heap object at {vaddr:#x} does not start on a page boundary"
            );
        }
        let end = paddr + size;
        if let Some(bad) = (paddr..end).step_by(page_size).find(|page| !is_untyped(*page)) {
            throw_unsup_format!(
                "KMiri: {size}-byte large heap object at {vaddr:#x} covers the page at paddr {bad:#x}, which is not an allocated untyped page"
            );
        }
        {
            let global_state = this.machine.alloc_addresses.borrow();
            let pos = global_state.int_to_ptr_map.partition_point(|(addr, _)| *addr < paddr as u64);
            if let Some((live, _)) = global_state.int_to_ptr_map.get(pos) {
                if *live < end as u64 {
                    throw_unsup_format!(
                        "KMiri: {size}-byte large heap object at {vaddr:#x} overlaps a live allocation at paddr {live:#x}"
                    );
                }
            }
        }
        this.kernel_allocation_at(paddr, size, page_size);
        interp_ok(true)
    }

    fn alloc_heap_slot(&mut self, ptr: Pointer, size: usize) -> InterpResult<'tcx, ()> {
        let this = self.eval_context_mut();
        if this.alloc_large_heap_object(ptr.addr().bytes(), size)? {
            return interp_ok(());
        }
        // A free slot is covered by the allocation that stands for it (see
        // `lazy_alloc_typed_slot_allocation`), e.g. because the free-list link
        // was written into it. End that allocation: the object replaces it.
        if let Some(free_id) = this.alloc_id_from_addr(ptr.addr().bytes(), 0) {
            if mirch::physical_mem_mut().free_slab_slots.remove(&free_id) {
                let info = this.get_alloc_info(free_id);
                // `ptr` may still carry the provenance of the object that last
                // lived in this slot, so free the slot through its address.
                this.deallocate_ptr(
                    Pointer::new(Some(Provenance::Wildcard), ptr.addr()),
                    Some((info.size, info.align)),
                    MemoryKind::Machine(MiriMemoryKind::Kernel),
                )?;
            }
        }
        // Look the slot up by address, so that a fresh allocation of `size`
        // bytes is created here even if `ptr` still carries the provenance of
        // the object that previously lived in this slot.
        mirch::physical_mem_mut().pending_heap_slot_size = Some(size);
        let alloc_id = this.alloc_id_from_addr(ptr.addr().bytes(), 0);
        let reused = mirch::physical_mem_mut().pending_heap_slot_size.take().is_some();
        let Some(alloc_id) = alloc_id else {
            throw_unsup_format!(
                "KMiri: heap slot at {:#x} is not in a typed slab page",
                ptr.addr().bytes()
            );
        };
        let live_size = this.get_alloc_info(alloc_id).size.bytes();
        if reused && live_size != size as u64 {
            throw_unsup_format!(
                "KMiri: heap slot at {:#x} already has a live {live_size}-byte allocation; cannot hand out a {size}-byte object",
                ptr.addr().bytes()
            );
        }
        interp_ok(())
    }

    /// Construct a new alloc when a fresh paddr is accessed the first time.
    fn kernel_allocation_at(&self, paddr: usize, size: usize, align: usize) -> AllocId {
        self.kernel_allocation_made_at(paddr, size, align, None)
    }

    fn kernel_allocation_made_at(
        &self,
        paddr: usize,
        size: usize,
        align: usize,
        made_at: Option<data_race::MadeAt>,
    ) -> AllocId {
        let ecx = self.eval_context_ref();
        let actual_paddr = paddr - paddr % align;
        let allocation = mirch::create_allocation_at(
            actual_paddr,
            Layout::from_size_align(size, align).unwrap(),
            ecx.machine.get_default_alloc_params(),
        );
        self.insert_kernel_allocation(paddr, actual_paddr, allocation, made_at)
    }

    /// Adds `allocation`, which lies at `actual_paddr`, as a kernel allocation. With
    /// `made_at`, it stands for memory written at that point rather than now.
    fn insert_kernel_allocation(
        &self,
        paddr: usize,
        actual_paddr: usize,
        allocation: Allocation<Provenance, (), MiriAllocBytes>,
        made_at: Option<data_race::MadeAt>,
    ) -> AllocId {
        let ecx = self.eval_context_ref();
        let alloc_id = ecx.tcx.reserve_alloc_id();
        let kind = rustc_const_eval::interpret::MemoryKind::Machine(MiriMemoryKind::Kernel);
        let allocation = {
            let mut extra = MiriMachine::init_allocation(
                ecx,
                alloc_id,
                kind,
                allocation.size(),
                allocation.align,
            )
            .unwrap();
            if let (Some(made_at), AllocDataRaceHandler::Vclocks(clocks, _)) =
                (made_at, &mut extra.data_race)
            {
                *clocks = data_race::AllocState::new_allocation_made_at(allocation.size(), made_at);
            }
            allocation.with_extra(extra)
        };
        ecx.memory.alloc_map().insert(alloc_id, (kind, allocation));
        {
            let mut global_state = ecx.machine.alloc_addresses.borrow_mut();
            global_state.set_exposed_kernel_padd(alloc_id, actual_paddr);
        }

        expose_tag(paddr, alloc_id, ecx);
        alloc_id
    }

    /// The physical address that the kernel's virtual address `vaddr` maps to.
    fn kernel_vaddr_to_paddr(&self, vaddr: u64) -> Option<u64> {
        let this = self.eval_context_ref();
        if this.machine.is_page_table_enabled() {
            let vaddr = vaddr as usize;
            let paddr_fallback = || {
                mirch::try_kernel_code_vaddr_to_paddr(vaddr)
                    .or_else(|| mirch::try_boot_pt_vaddr_to_paddr(vaddr))
            };
            // An address that neither the page table nor the two linear mappings cover
            // belongs to no allocation, e.g. a CPU-local variable addressed through a
            // base register that is still 0 (Asterinas #1069). Report it as dangling.
            Some(mirch::page_walk_or(vaddr, || unreachable!()).or_else(paddr_fallback)? as u64)
        } else {
            let vaddr_usize = vaddr as usize;
            let paddr = mirch::try_kernel_code_vaddr_to_paddr(vaddr_usize)
                .or_else(|| mirch::try_boot_pt_vaddr_to_paddr(vaddr_usize))
                .unwrap_or(vaddr_usize) as u64;
            log!("vaddr -> paddr (no-pt) = {vaddr:#x} -> {paddr:#x}");
            Some(paddr)
        }
    }

    /// If `paddr` lies in a copy of the CPU-local section, returns `Some` with the copy of
    /// the variable that covers it, creating that copy on first access, or `Some(None)` if
    /// no variable covers it. Returns `None` if `paddr` is in no copy of the section.
    fn cpu_local_copy_at(&self, paddr: usize) -> Option<Option<AllocId>> {
        let this = self.eval_context_ref();
        let section_start = mirch::cpu_local_start_addr();
        let section_len = mirch::cpu_local_segment_size();
        let (base, made_at) = {
            let copies = this.machine.cpu_local_copies.borrow();
            let copy = copies.iter().find(|c| (c.base..c.base + section_len).contains(&paddr))?;
            (copy.base, copy.made_at)
        };
        let in_section = section_start + (paddr - base);
        // The variable at the same offset in the section.
        let (var_paddr, var_id) = {
            let global_state = this.machine.alloc_addresses.borrow();
            let map = &global_state.int_to_ptr_map;
            let pos = map.partition_point(|&(addr, _)| addr <= in_section as u64);
            if pos == 0 {
                return Some(None);
            }
            map[pos - 1]
        };
        if !this.machine.cpu_local_alloc_set.borrow().contains(&var_id) {
            return Some(None);
        }
        let size = this.get_alloc_info(var_id).size;
        if in_section as u64 - var_paddr >= size.bytes() {
            return Some(None);
        }
        let copy_paddr = base + (var_paddr as usize - section_start);
        {
            // A variable copied with the section has its copy already; so does one that
            // was accessed in this copy before. `paddr` is then outside it.
            let copies = this.machine.cpu_local_copies.borrow();
            let copy = copies.iter().find(|c| c.base == base).unwrap();
            let global_state = this.machine.alloc_addresses.borrow();
            if copy.copied.contains(&var_id)
                || global_state
                    .int_to_ptr_map
                    .binary_search_by_key(&(copy_paddr as u64), |(addr, _)| *addr)
                    .is_ok()
            {
                return Some(None);
            }
        }
        // The variable was not used before the section was copied, so the copy holds its
        // initial value.
        let GlobalAlloc::Static(def_id) = this.tcx.global_alloc(var_id) else {
            return Some(None);
        };
        let initial = this.tcx.eval_static_initializer(def_id).expect("static has a value");
        let initial = initial
            .inner()
            .adjust_from_tcx(
                &this.tcx,
                |bytes, align| this.get_global_alloc_bytes(var_id, bytes, align),
                |ptr| this.global_root_pointer(ptr),
            )
            .discard_err()?;
        let mut allocation = crate::machine::place_allocation_at(this, copy_paddr, &initial);
        allocation.mutability = rustc_ast::Mutability::Mut;
        let alloc_id = self.insert_kernel_allocation(copy_paddr, copy_paddr, allocation, made_at);
        log!("[cpu_local_copy_at] copy of {var_id:?} at {copy_paddr:#x}: {alloc_id:?}");
        Some(Some(alloc_id))
    }

    /// Copies the CPU-local section to the virtual address `dst`, as `memcpy` would, but
    /// with one allocation per variable, so that each copy keeps its provenance, borrow
    /// state, and race detection. Variables that were used before the copy are copied now;
    /// the others are copied, with their initial value, on first access to the copy.
    fn copy_cpu_local_section(&mut self, dst: usize) -> InterpResult<'tcx> {
        let this = self.eval_context_mut();
        let page_size = mirch::page_size();
        let section_start = mirch::cpu_local_start_addr();
        let pages = mirch::cpu_local_segment_size().div_ceil(page_size);
        let Some(dst_paddr) = this.kernel_vaddr_to_paddr(dst as u64) else {
            throw_ub_format!("copying the CPU-local section to unmapped address {dst:#x}");
        };
        let dst_paddr = dst_paddr as usize;
        // On hardware every variable keeps its alignment in a copy at a page-aligned
        // address. KMiri places each copy at the variable's offset, so it needs that too.
        if !dst.is_multiple_of(page_size)
            || !dst_paddr.abs_diff(section_start).is_multiple_of(page_size)
        {
            throw_unsup_format!(
                "KMiri only supports copies of the CPU-local section at a page boundary"
            );
        }
        for i in 0..pages {
            let page = this.kernel_vaddr_to_paddr((dst + i * page_size) as u64);
            if page != Some((dst_paddr + i * page_size) as u64) {
                throw_unsup_format!(
                    "KMiri only supports copies of the CPU-local section to contiguous physical memory"
                );
            }
            mirch::check_page_state(dst_paddr + i * page_size, PageState::Untyped)?;
        }
        mirch::type_pages_at(dst_paddr, pages, page_size, TypedKind::CpuLocalCopy)?;

        let made_at = match &this.machine.data_race {
            GlobalDataRaceHandler::Vclocks(data_race) =>
                Some(data_race.active_thread_now(&this.machine.threads)),
            _ => None,
        };
        let mut used: Vec<AllocId> =
            this.machine.cpu_local_alloc_set.borrow().iter().copied().collect();
        used.sort();
        let mut copied = FxHashSet::default();
        for var_id in used {
            let info = this.get_alloc_info(var_id);
            if info.size.bytes() == 0 {
                continue;
            }
            let var_vaddr = this.addr_from_alloc_id(var_id, Some(MiriMemoryKind::Global.into()))?;
            let var_paddr = this.machine.alloc_addresses.borrow().get_base_addr(var_id) as usize;
            let offset = var_paddr - section_start;
            this.kernel_allocation_at(
                dst_paddr + offset,
                info.size.bytes_usize(),
                info.align.bytes_usize(),
            );
            this.mem_copy(
                interpret::Pointer::new(Some(Provenance::Wildcard), Size::from_bytes(var_vaddr)),
                interpret::Pointer::new(Some(Provenance::Wildcard), Size::from_bytes(dst + offset)),
                info.size,
                /*nonoverlapping*/ true,
            )?;
            copied.insert(var_id);
        }
        this.machine.cpu_local_copies.borrow_mut().push(CpuLocalCopy {
            base: dst_paddr,
            copied,
            made_at,
        });
        interp_ok(())
    }

    // Returns the `AllocId` that corresponds to the specified addr,
    // or `None` if the addr is out of bounds.
    fn alloc_id_from_addr(&self, vaddr: u64, size: i64) -> Option<AllocId> {
        log!("[alloc_id_from_addr] vaddr={vaddr:#x} size={size}");
        let ecx = self.eval_context_ref();
        let global_state = ecx.machine.alloc_addresses.borrow();
        assert!(global_state.provenance_mode != ProvenanceMode::Strict);

        // vaddr to paddr

        // unwrap_or(false) when asterinas migrates to toml config.
        let page_table_enabled = ecx.machine.is_page_table_enabled();
        let paddr = ecx.kernel_vaddr_to_paddr(vaddr)?;

        // We always search the allocation to the right of this address. So if the size is strictly
        // negative, we have to search for `addr-1` instead.
        let addr = if size >= 0 { paddr } else { paddr.saturating_sub(1) };
        let pos = global_state.int_to_ptr_map.binary_search_by_key(&addr, |(addr, _)| *addr);

        // Determine the in-bounds provenance for this pointer.
        let alloc_id = match pos {
            Ok(pos) => Some(global_state.int_to_ptr_map[pos].1),
            Err(0) => {
                // If cannot found, first check whether the allocation is a lazy allocated one (typed slot).
                let paddr = paddr as usize;
                log!(
                    "[alloc_id_from_addr - lazy_alloc_typed_slot_allocation] no alloc_id pos=Err(0) paddr={paddr:#x}"
                );
                drop(global_state);
                match self.cpu_local_copy_at(paddr) {
                    Some(copy) => copy,
                    None => self.lazy_alloc_typed_slot_allocation(paddr),
                }
            }
            Err(pos) => {
                // This is the largest of the addresses smaller than `int`,
                // i.e. the greatest lower bound (glb)
                let (glb, alloc_id) = global_state.int_to_ptr_map[pos - 1];
                // This never overflows because `addr >= glb`
                let offset = addr - glb;
                // We require this to be strict in-bounds of the allocation. This arm is only
                // entered for addresses that are not the base address, so even zero-sized
                // allocations will get recognized at their base address -- but all other
                // allocations will *not* be recognized at their "end" address.
                let size = ecx.get_alloc_info(alloc_id).size;
                if offset < size.bytes() {
                    Some(alloc_id)
                } else {
                    // FIXME: explain the kmiri logic in this branch
                    let paddr = paddr as usize;
                    drop(global_state);
                    if let Some(copy) = self.cpu_local_copy_at(paddr) {
                        copy
                    } else if page_table_enabled {
                        self.lazy_alloc_typed_slot_allocation(paddr)
                    } else {
                        let kalloc =
                            ecx.machine.kmiri_toml.as_ref().map(|val| val.get_kalloc(paddr))??;
                        let alloc_id =
                            self.kernel_allocation_at(kalloc.base_addr, kalloc.size, kalloc.align);
                        log!("kernel_allocation_at: {paddr:#x} {alloc_id:?}");
                        Some(alloc_id)
                    }
                }
            }
        }?;

        // We only use this provenance if it has been exposed.
        if ecx.machine.alloc_addresses.borrow().exposed.contains(&alloc_id) {
            // This must still be live, since we remove allocations from `int_to_ptr_map` when they get freed.
            debug_assert!(ecx.is_alloc_live(alloc_id));
            Some(alloc_id)
        } else {
            None
        }
    }

    /// Returns the base address of an allocation, or an error if no base address could be found
    ///
    /// # Panics
    /// If `memory_kind = None` and the `alloc_id` is not cached, meaning that the first call to this function per `alloc_id` must get the `memory_kind`.
    fn addr_from_alloc_id(
        &self,
        alloc_id: AllocId,
        memory_kind: Option<MemoryKind>,
    ) -> InterpResult<'tcx, u64> {
        let ecx = self.eval_context_ref();
        let mut global_state = ecx.machine.alloc_addresses.borrow_mut();

        let vaddr = match global_state.base_paddr.get(&alloc_id) {
            Some(&paddr) =>
                global_state
                    .paddr_to_vaddr
                    .get(&paddr)
                    .cloned()
                    .unwrap_or_else(|| kernel_code_paddr_to_vaddr(paddr as usize) as u64),
            None => {
                if let Some(&vaddr) = global_state.layout_fn_vaddr.get(&alloc_id) {
                    return interp_ok(vaddr);
                }
                if let Some(link_name) = MiriMachine::unresolved_foreign_fn_symbol(ecx, alloc_id)?
                    && let Some(&vaddr) = ecx.machine.foreign_symbol_addr_map.get(&link_name)
                {
                    global_state.layout_fn_vaddr.insert(alloc_id, vaddr);
                    return interp_ok(vaddr);
                }

                // First time we're looking for the absolute address of this allocation.
                let memory_kind =
                    memory_kind.expect("memory_kind is required since alloc_id is not cached");
                let base_vaddr =
                    ecx.addr_from_alloc_id_uncached(&mut global_state, alloc_id, memory_kind)?;

                // The addr is 0 for TypeId AllocKind.
                if base_vaddr == 0 {
                    return interp_ok(0);
                }

                // kmiri: vaddr to paddr; or just base address if not appropriate
                let paddr_fallback = || {
                    // log!(
                    //     "[addr_from_alloc_id - page_walk_or] vaddr={base_vaddr:#x} ({alloc_id:?})"
                    // );
                    let base_vaddr = base_vaddr as usize;
                    mirch::try_kernel_code_vaddr_to_paddr(base_vaddr).unwrap_or_else(|| {
                        panic!("{base_vaddr:#x} ({alloc_id:?}) is not in kernel memory region")
                    })
                };
                let base_paddr = mirch::page_walk_or(base_vaddr as usize, paddr_fallback)
                    .unwrap_or_else(paddr_fallback) as u64;

                {
                    if ecx.machine.threads.active_thread().to_u32() == 1 {
                        // log!(
                        //     "[addr_from_alloc_id] paddr={base_paddr:#x} vaddr={base_vaddr:#x} ({alloc_id:?})"
                        // );
                        global_state.paddr_to_vaddr.insert(base_paddr, base_vaddr);
                    }
                }

                // Store address in cache.
                global_state.base_paddr.try_insert(alloc_id, base_paddr).unwrap();

                // Also maintain the opposite mapping in `int_to_ptr_map`, ensuring we keep it
                // sorted. We have a fast-path for the common case that this address is bigger than
                // all previous ones. We skip this for allocations at address 0; those can't be
                // real, they must be TypeId "fake allocations".
                if base_paddr != 0 {
                    let pos = if global_state
                        .int_to_ptr_map
                        .last()
                        .is_some_and(|(last_addr, _)| *last_addr < base_paddr)
                    {
                        global_state.int_to_ptr_map.len()
                    } else {
                        match global_state
                            .int_to_ptr_map
                            .binary_search_by_key(&base_paddr, |(addr, _)| *addr)
                        {
                            Ok(found) => {
                                let found_alloc_id = global_state.int_to_ptr_map[found].1;
                                assert_ne!(
                                    found_alloc_id, alloc_id,
                                    "{base_paddr} has two AllocId {alloc_id:?} and {found_alloc_id:?}"
                                );
                                return interp_ok(base_paddr);
                            }
                            Err(pos) => pos,
                        }
                    };
                    global_state.int_to_ptr_map.insert(pos, (base_paddr, alloc_id));

                    drop(global_state);
                    if mirch::is_in_cpu_local_section(base_paddr as usize) {
                        log!("expose cpu local alloc: {alloc_id:?}");
                        expose_tag(base_paddr as usize, alloc_id, ecx);
                    }
                }

                base_vaddr
            }
        };
        interp_ok(vaddr)
    }

    fn expose_provenance(&self, provenance: Provenance) -> InterpResult<'tcx> {
        let ecx = self.eval_context_ref();
        let mut global_state = ecx.machine.alloc_addresses.borrow_mut();

        let (alloc_id, tag) = match provenance {
            Provenance::Concrete { alloc_id, tag } => (alloc_id, tag),
            Provenance::Wildcard => {
                // No need to do anything for wildcard pointers as
                // their provenances have already been previously exposed.
                return interp_ok(());
            }
        };

        // In strict mode, we don't need this, so we can save some cycles by not tracking it.
        if global_state.provenance_mode == ProvenanceMode::Strict {
            return interp_ok(());
        }
        // Exposing a dead alloc is a no-op, because it's not possible to get a dead allocation
        // via int2ptr.
        if !ecx.is_alloc_live(alloc_id) {
            return interp_ok(());
        }
        trace!("Exposing allocation id {alloc_id:?}");
        global_state.exposed.insert(alloc_id);
        // Release the global state before we call `expose_tag`, which may call `get_alloc_info_extra`,
        // which may need access to the global state.
        drop(global_state);
        if ecx.machine.borrow_tracker.is_some() {
            ecx.expose_tag(alloc_id, tag)?;
        }
        interp_ok(())
    }

    fn ptr_from_addr_cast(&self, addr: u64) -> InterpResult<'tcx, Pointer> {
        log!("Casting {:#x} to a pointer", addr);

        let ecx = self.eval_context_ref();
        let global_state = ecx.machine.alloc_addresses.borrow();

        // Potentially emit a warning.
        match global_state.provenance_mode {
            ProvenanceMode::Default => {
                // The first time this happens at a particular location, print a warning.
                static DEDUP: SpanDedupDiagnostic = SpanDedupDiagnostic::new();
                ecx.dedup_diagnostic(&DEDUP, |first| {
                    NonHaltingDiagnostic::Int2Ptr { details: first }
                });
            }
            ProvenanceMode::Strict => {
                throw_machine_stop!(TerminationInfo::Int2PtrWithStrictProvenance);
            }
            ProvenanceMode::Permissive => {}
        }

        // We do *not* look up the `AllocId` here! This is a `usize as ptr` cast, and it is
        // completely legal to do a cast and then `wrapping_offset` to another allocation and only
        // *then* do a memory access. So the allocation that the pointer happens to point to on a
        // cast is fairly irrelevant. Instead we generate this as a "wildcard" pointer, such that
        // *every time the pointer is used*, we do an `AllocId` lookup to find the (exposed)
        // allocation it might be referencing.
        interp_ok(Pointer::new(Some(Provenance::Wildcard), Size::from_bytes(addr)))
    }

    /// Convert a relative (tcx) pointer to a Miri pointer.
    fn adjust_alloc_root_pointer(
        &self,
        ptr: interpret::Pointer<CtfeProvenance>,
        tag: BorTag,
        kind: MemoryKind,
    ) -> InterpResult<'tcx, interpret::Pointer<Provenance>> {
        let ecx = self.eval_context_ref();

        let (prov, offset) = ptr.prov_and_relative_offset();
        let alloc_id = prov.alloc_id();

        // Get a pointer to the beginning of this allocation.
        let base_addr = ecx.addr_from_alloc_id(alloc_id, Some(kind))?;

        // kmiri: vaddr to paddr
        // kmiri: replace the stack allocation by pointing to the kernel stack region
        if kind == MemoryKind::Stack {
            let base_paddr = {
                let global_state = ecx.machine.alloc_addresses.borrow();
                *global_state.base_paddr.get(&alloc_id).unwrap()
            };
            let alloc_map = &ecx.memory.alloc_map();
            let (kind, old_allocation) = &alloc_map.get(alloc_id).unwrap();
            let alloc_size_usize = old_allocation.size().bytes_usize();
            if alloc_size_usize > 0 {
                let (new_allocation, kind) = {
                    let mut allocation = mirch::create_allocation_at(
                        base_paddr as usize,
                        std::alloc::Layout::from_size_align(
                            old_allocation.size().bytes_usize(),
                            old_allocation.align.bytes_usize(),
                        )
                        .unwrap(),
                        ecx.machine.get_default_alloc_params(),
                    );
                    let extra = MiriMachine::init_allocation(
                        ecx,
                        alloc_id,
                        *kind,
                        old_allocation.size(),
                        old_allocation.align,
                    )?;

                    let alloc_range = rustc_middle::mir::interpret::alloc_range(
                        Size::ZERO,
                        old_allocation.size(),
                    );
                    let init_mask = old_allocation.init_mask();

                    // Preserve unknown bytes when moving a stack allocation
                    // onto physical backing, even if every byte is uninitialized.
                    let init_copy = init_mask.prepare_copy((0..alloc_size_usize).into());
                    allocation.init_mask_apply_copy(init_copy, alloc_range, 1);

                    if !init_mask.is_range_initialized(alloc_range).is_err_and(|range| {
                        range.start == alloc_range.start && range.size == alloc_range.size
                    }) {
                        // Copy context
                        let src_ptr = old_allocation.get_bytes_unchecked_raw();
                        let dst_ptr = allocation.get_bytes_unchecked_raw_mut();
                        unsafe {
                            core::ptr::copy(src_ptr, dst_ptr, alloc_size_usize);
                        }

                        // Copy provenance
                        let provenance_copy =
                            old_allocation.provenance().prepare_copy(alloc_range, &[0], ecx);
                        allocation.provenance_apply_copy(provenance_copy, alloc_range, 1);
                    }
                    (allocation.with_extra(extra), *kind)
                };

                alloc_map.insert(alloc_id, (kind, new_allocation));
            }
        }

        let base_ptr = interpret::Pointer::new(
            Provenance::Concrete { alloc_id, tag },
            Size::from_bytes(base_addr),
        );
        // Add offset with the right kind of pointer-overflowing arithmetic.
        interp_ok(base_ptr.wrapping_offset(offset, ecx))
    }

    // This returns some prepared `MiriAllocBytes`, either because `addr_from_alloc_id` reserved
    // memory space in the past, or by doing the pre-allocation right upon being called.
    fn get_global_alloc_bytes(
        &self,
        id: AllocId,
        bytes: &[u8],
        align: Align,
    ) -> InterpResult<'tcx, MiriAllocBytes> {
        let ecx = self.eval_context_ref();
        assert!(ecx.tcx.try_get_global_alloc(id).is_some());
        if !ecx.machine.native_lib.is_empty() {
            // In native lib mode, MiriAllocBytes for global allocations are handled via `prepared_alloc_bytes`.
            // This additional call ensures that some `MiriAllocBytes` are always prepared, just in case
            // this function gets called before the first time `addr_from_alloc_id` gets called.
            ecx.addr_from_alloc_id(id, Some(MiriMemoryKind::Global.into()))?;
            // The memory we need here will have already been allocated during an earlier call to
            // `addr_from_alloc_id` for this allocation. So don't create a new `MiriAllocBytes` here, instead
            // fetch the previously prepared bytes from `prepared_alloc_bytes`.
            let mut global_state = ecx.machine.alloc_addresses.borrow_mut();
            let mut prepared_alloc_bytes = global_state
                .prepared_alloc_bytes
                .as_mut()
                .unwrap()
                .remove(&id)
                .unwrap_or_else(|| panic!("alloc bytes for {id:?} have not been prepared"));
            // Sanity-check that the prepared allocation has the right size and alignment.
            assert!(prepared_alloc_bytes.as_ptr().is_aligned_to(align.bytes_usize()));
            assert_eq!(prepared_alloc_bytes.len(), bytes.len());
            // Copy allocation contents into prepared memory.
            prepared_alloc_bytes.copy_from_slice(bytes);
            interp_ok(prepared_alloc_bytes)
        } else {
            let params = ecx.machine.get_default_alloc_params();
            interp_ok(MiriAllocBytes::from_bytes(std::borrow::Cow::Borrowed(bytes), align, params))
        }
    }

    /// When a pointer is used for a memory access, this computes where in which allocation the
    /// access is going.
    fn ptr_get_alloc(
        &self,
        ptr: interpret::Pointer<Provenance>,
        size: i64,
    ) -> Option<(AllocId, Size)> {
        let ecx = self.eval_context_ref();

        let (tag, vaddr) = ptr.into_raw_parts(); // addr is absolute (Miri provenance)

        let alloc_id = if let Provenance::Concrete { alloc_id, .. } = tag {
            alloc_id
        } else {
            // A wildcard pointer.
            ecx.alloc_id_from_addr(vaddr.bytes(), size)?
        };

        let info = ecx.get_alloc_info(alloc_id);
        if info.kind == AllocKind::TypeId {
            return Some((alloc_id, vaddr));
        }

        // A `[layout]` function: the address is the symbol value, there is no memory to walk.
        if let Some(&base) = ecx.machine.alloc_addresses.borrow().layout_fn_vaddr.get(&alloc_id) {
            return Some((alloc_id, Size::from_bytes(vaddr.bytes().wrapping_sub(base))));
        }

        // This cannot fail: since we already have a pointer with that provenance, adjust_alloc_root_pointer
        // must have been called in the past, so we can just look up the address in the map.
        let base_paddr = *ecx.machine.alloc_addresses.borrow().base_paddr.get(&alloc_id).unwrap();

        let vaddr = vaddr.bytes_usize();
        let boot_pt = false;
        let paddr_fallback = || {
            // kernel_code_vaddr_to_paddr(addr.bytes_usize())
            mirch::try_kernel_code_vaddr_to_paddr(vaddr).unwrap_or_else(|| {
                let kind = ecx
                    .memory
                    .alloc_map()
                    .get(alloc_id)
                    .unwrap_or_else(|| panic!("{alloc_id:?} vaddr={vaddr:#x} is not in alloc_map"))
                    .0;
                log!("vaddr={vaddr:#x} alloc_id={alloc_id:?} kind={kind:?}");
                // boot_pt = true;
                mirch::try_boot_pt_vaddr_to_paddr(vaddr).unwrap_or(vaddr)
            })
        };
        let actual_paddr =
            mirch::page_walk_or(vaddr, paddr_fallback).unwrap_or_else(paddr_fallback) as u64;
        if boot_pt {
            log!("[ptr_get_alloc] boot_pt paddr={actual_paddr:#x} size={size}");
        }

        let offset = actual_paddr.wrapping_sub(base_paddr);

        // Wrapping "addr - base_addr"
        let rel_offset = ecx.truncate_to_target_usize(offset);
        Some((alloc_id, Size::from_bytes(rel_offset)))
    }

    /// Return a list of all exposed allocations.
    fn exposed_allocs(&self) -> Vec<AllocId> {
        let ecx = self.eval_context_ref();
        ecx.machine.alloc_addresses.borrow().exposed.iter().copied().collect()
    }
}

fn expose_tag<'tcx>(paddr: usize, alloc_id: AllocId, ecx: &InterpCx<'tcx, MiriMachine<'tcx>>) {
    let Some(borrow_tracker) = ecx.machine.borrow_tracker.as_ref() else {
        // Without a borrow tracker (`-Zmiri-disable-stacked-borrows`) there is no
        // tag to expose; marking the allocation exposed is all int2ptr needs.
        ecx.machine.alloc_addresses.borrow_mut().exposed.insert(alloc_id);
        return;
    };
    // Re-expose the root tag so wildcard/raw-pointer accesses can find a
    // writable provenance after the typed-slot allocation is created.
    let root_tag = borrow_tracker.borrow_mut().root_ptr_tag(alloc_id, &ecx.machine);
    ecx.machine.alloc_addresses.borrow_mut().exposed.insert(alloc_id);
    ecx.expose_tag(alloc_id, root_tag).discard_err().unwrap_or_else(|| {
        panic!("paddr={paddr:#x} ({alloc_id:?}) with {root_tag:?} has been exposed")
    });
}

impl<'tcx> MiriMachine<'tcx> {
    pub fn free_alloc_id(&mut self, dead_id: AllocId, size: Size, align: Align, kind: MemoryKind) {
        let global_state = self.alloc_addresses.get_mut();
        let rng = self.rng.get_mut();

        // We can *not* remove this from `base_addr`, since the interpreter design requires that we
        // be able to retrieve an AllocId + offset for any memory access *before* we check if the
        // access is valid. Specifically, `ptr_get_alloc` is called on each attempt at a memory
        // access to determine the allocation ID and offset -- and there can still be pointers with
        // `dead_id` that one can attempt to use for a memory access. `ptr_get_alloc` may return
        // `None` only if the pointer truly has no provenance (this ensures consistent error
        // messages).
        // However, we *can* remove it from `int_to_ptr_map`, since any wildcard pointers that exist
        // can no longer actually be accessing that address. This ensures `alloc_id_from_addr` never
        // returns a dead allocation.
        // To avoid a linear scan we first look up the address in `base_addr`, and then find it in
        // `int_to_ptr_map`.
        let paddr = *global_state.base_paddr.get(&dead_id).unwrap();
        let Ok(pos) = global_state.int_to_ptr_map.binary_search_by_key(&paddr, |(addr, _)| *addr)
        else {
            panic!("paddr={paddr:#x} is not in int_to_ptr_map");
        };
        let removed = global_state.int_to_ptr_map.remove(pos);
        assert_eq!(removed, (paddr, dead_id)); // double-check that we removed the right thing
        // We can also remove it from `exposed`, since this allocation can anyway not be returned by
        // `alloc_id_from_addr` any more.
        global_state.exposed.remove(&dead_id);
        // Also remember this address for future reuse.
        if let Some((_addr_gen, reuse)) = global_state.address_generation.as_mut() {
            let thread = self.threads.active_thread();
            reuse.add_addr(rng, paddr, size, align, kind, thread, || {
                // We cannot be in GenMC mode as then `address_generation` is `None`. We cannot use
                // `self.release_clock` as `self.alloc_addresses` is borrowed.
                if let Some(data_race) = self.data_race.as_vclocks_ref() {
                    data_race.release_clock(&self.threads, |clock| clock.clone())
                } else {
                    VClock::default()
                }
            })
        }
    }
}
