//! Objects that a kernel's own allocator hands out from a memory pool.
//!
//! A `[[pool]]` in the KMiri config is memory that an allocator of the kernel
//! manages. The allocator runs unchanged and reaches the pool through
//! addresses. At the boundary of the allocator, the kernel tells KMiri which
//! blocks it hands out and which it takes back:
//!
//! - `kern_miri_object_born(ptr, size, align)`, after the allocator returned a
//!   block: KMiri makes an allocation of exactly `size` uninitialized bytes at
//!   `ptr` and returns a pointer to it, which the kernel gives to its caller.
//! - `kern_miri_object_died(ptr)`, before the block goes back to the allocator:
//!   the allocation ends and the pool bytes it covered become uninitialized.
//!   The returned pointer only carries the address; the allocator uses it to
//!   reach the free block in the pool.
//! - `kern_miri_object_realloc_begin(ptr)` and
//!   `kern_miri_object_realloc_end(old, new, size, align)` around a
//!   reallocation. In between, the old object is left out of address lookups,
//!   so the allocator reaches the block through the pool, which then shows what
//!   the object holds (as the memory does natively). `begin` returns a pointer
//!   that only carries the address, for the kernel to pass to its allocator, as
//!   `kern_miri_object_died` does. On success the old
//!   object ends and the new one keeps the initialization state and provenance
//!   of the first `min(old size, new size)` bytes of the old one; on failure
//!   (`new` is null) the old object stays.
//!
//! An address resolves to the innermost live allocation: a local variable, the
//! object holding it, and only then the pool.
//!
//! The memory of an object may hold the stack of a thread. When that memory is
//! freed, the thread ends without running again; freeing it while the thread
//! runs on a CPU is reported.

use std::alloc::Layout;
use std::ops::Range;

use rustc_abi::Size;
use rustc_middle::mir::interpret::alloc_range;

use super::expose_tag;
use crate::*;

impl<'tcx> EvalContextExt<'tcx> for crate::MiriInterpCx<'tcx> {}
pub trait EvalContextExt<'tcx>: crate::MiriInterpCxExt<'tcx> {
    /// The allocation that stands for the pool holding `paddr`, made on first use.
    fn pool_alloc_at(&self, paddr: u64) -> Option<AllocId> {
        let ecx = self.eval_context_ref();
        let pool = ecx.machine.kmiri_toml.as_ref()?.pool_containing(paddr as usize)?;
        let base = pool.base_addr as u64;
        if let Some(id) = ecx.machine.alloc_addresses.borrow().pool_allocs.get(&base) {
            return Some(*id);
        }
        let mut allocation = mirch::create_allocation_at(
            pool.base_addr,
            Layout::from_size_align(pool.size, mirch::page_size()).unwrap(),
            ecx.machine.get_default_alloc_params(),
        );
        // Nothing has been written to the pool yet.
        allocation.write_uninit(ecx, alloc_range(Size::ZERO, Size::from_bytes(pool.size)));
        let alloc_id = ecx.tcx.reserve_alloc_id();
        let kind = MemoryKind::Machine(MiriMemoryKind::Pool);
        let extra =
            MiriMachine::init_allocation(ecx, alloc_id, kind, allocation.size(), allocation.align)
                .unwrap();
        ecx.memory.alloc_map().insert(alloc_id, (kind, allocation.with_extra(extra)));
        {
            let mut global_state = ecx.machine.alloc_addresses.borrow_mut();
            // Not in `int_to_ptr_map`: the pool is only the last resort of an
            // address lookup, after every allocation placed inside it.
            global_state.base_paddr.insert(alloc_id, base);
            global_state.exposed.insert(alloc_id);
            global_state.pool_allocs.insert(base, alloc_id);
        }
        expose_tag(pool.base_addr, alloc_id, ecx);
        Some(alloc_id)
    }

    /// The live object that holds `paddr`: its base, size and allocation.
    fn live_object_at(&self, paddr: u64) -> Option<(u64, u64, AllocId)> {
        let global_state = self.eval_context_ref().machine.alloc_addresses.borrow();
        let (&base, &(size, id)) = global_state.objects.range(..=paddr).next_back()?;
        (paddr - base < size).then_some((base, size, id))
    }

    /// The last resort of an address lookup: the object, else the pool, that
    /// holds `paddr`.
    fn kernel_object_or_pool_at(&self, paddr: u64) -> Option<AllocId> {
        if let Some((_, _, id)) = self.live_object_at(paddr) {
            return Some(id);
        }
        self.pool_alloc_at(paddr)
    }

    /// `kern_miri_object_born`: the allocator handed out `size` bytes at `ptr`.
    fn kernel_object_born(
        &mut self,
        ptr: Pointer,
        size: u64,
        align: u64,
    ) -> InterpResult<'tcx, Pointer> {
        let this = self.eval_context_mut();
        let addr = ptr.addr().bytes();
        let paddr = this.object_paddr(addr)?;
        this.check_new_object(addr, paddr, size, align)?;
        let allocation = this.new_object_allocation(paddr, size, align);
        interp_ok(this.insert_object(addr, paddr, size, allocation))
    }

    /// `kern_miri_object_died`: the object at `ptr` goes back to the allocator.
    fn kernel_object_died(&mut self, ptr: Pointer) -> InterpResult<'tcx, Pointer> {
        let this = self.eval_context_mut();
        let addr = ptr.addr().bytes();
        let (paddr, size, alloc_id) = this.take_live_object(ptr, "freeing")?;
        this.end_threads_on_stack(addr..addr + size)?;
        this.mark_pool_uninit(paddr, size)?;
        this.free_object(ptr, alloc_id, size)?;
        interp_ok(Pointer::new(Some(Provenance::Wildcard), Size::from_bytes(addr)))
    }

    /// `kern_miri_object_realloc_begin`: the allocator is about to reallocate
    /// the object at `ptr`. Returns the pointer for the allocator, which only
    /// carries the address.
    fn kernel_object_realloc_begin(&mut self, ptr: Pointer) -> InterpResult<'tcx, Pointer> {
        let this = self.eval_context_mut();
        let addr = ptr.addr().bytes();
        let (paddr, size, alloc_id) = this.take_live_object(ptr, "reallocating")?;
        {
            let mut global_state = this.machine.alloc_addresses.borrow_mut();
            global_state.unmap_addr(paddr, alloc_id);
            global_state.suspended_objects.insert(paddr, (size, alloc_id));
        }
        // The allocator now reaches the block through the pool: show it what
        // the object holds, so that copying through the pool moves the
        // contents (a copy of uninitialized bytes would not even move them).
        this.show_object_in_pool(paddr, size, alloc_id)?;
        interp_ok(Pointer::new(Some(Provenance::Wildcard), Size::from_bytes(addr)))
    }

    /// `kern_miri_object_realloc_end`: the allocator reallocated the object at
    /// `old` to `size` bytes at `new`, or failed if `new` is null. Returns the
    /// pointer for the caller: to the new object, or null.
    fn kernel_object_realloc_end(
        &mut self,
        old: Pointer,
        new: Pointer,
        size: u64,
        align: u64,
    ) -> InterpResult<'tcx, Pointer> {
        let this = self.eval_context_mut();
        let old_addr = old.addr().bytes();
        let old_paddr = this.object_paddr(old_addr)?;
        let Some((old_size, old_id)) =
            this.machine.alloc_addresses.borrow_mut().suspended_objects.remove(&old_paddr)
        else {
            throw_unsup_format!(
                "KMiri: `kern_miri_object_realloc_end` for {old_addr:#x} without `kern_miri_object_realloc_begin`"
            );
        };
        {
            let mut global_state = this.machine.alloc_addresses.borrow_mut();
            global_state.set_exposed_kernel_padd(old_id, old_paddr as usize);
            if new.addr().bytes() == 0 {
                // The reallocation failed: the old object stays.
                global_state.objects.insert(old_paddr, (old_size, old_id));
                return interp_ok(Pointer::null());
            }
        }
        let new_addr = new.addr().bytes();
        let new_paddr = this.object_paddr(new_addr)?;

        // The new object keeps what the old one held, as far as both reach.
        // Take it from the old object itself rather than from the copy the
        // allocator made.
        let kept = old_size.min(size);
        let contents = (kept > 0).then(|| {
            let ecx = &*this;
            let old_alloc = ecx.get_alloc_raw(old_id).unwrap();
            let range = alloc_range(Size::ZERO, Size::from_bytes(kept));
            (
                old_alloc.init_mask().prepare_copy(range),
                old_alloc.provenance_prepare_copy(range, ecx),
            )
        });
        // A stack in the part of the old block that the new one does not
        // cover is gone; one in the part they share stays where it is.
        let old_end = old_addr + old_size;
        let new_end = new_addr.saturating_add(size);
        this.end_threads_on_stack(old_addr..old_end.min(new_addr))?;
        this.end_threads_on_stack(old_addr.max(new_end)..old_end)?;
        this.free_object(old, old_id, old_size)?;
        // The pool under the old block stays as the allocator left it: it may
        // have linked the block into a free list in between.

        this.check_new_object(new_addr, new_paddr, size, align)?;
        let mut allocation = this.new_object_allocation(new_paddr, size, align);
        if let Some((init, provenance)) = contents {
            let range = alloc_range(Size::ZERO, Size::from_bytes(kept));
            allocation.init_mask_apply_copy(init, range, 1);
            allocation.provenance_apply_copy(provenance, range, 1);
        }
        interp_ok(this.insert_object(new_addr, new_paddr, size, allocation))
    }

    /// The physical address of the block at the kernel address `addr`.
    fn object_paddr(&self, addr: u64) -> InterpResult<'tcx, u64> {
        match self.eval_context_ref().kernel_vaddr_to_paddr(addr) {
            Some(paddr) => interp_ok(paddr),
            None =>
                throw_ub_format!(
                    "the allocator used the block at {addr:#x}, which is not in memory"
                ),
        }
    }

    /// Checks that a block the allocator hands out lies in a pool, is aligned,
    /// and overlaps no live object.
    fn check_new_object(&self, addr: u64, paddr: u64, size: u64, align: u64) -> InterpResult<'tcx> {
        let ecx = self.eval_context_ref();
        if !align.is_power_of_two() {
            throw_unsup_format!(
                "KMiri: a kernel object with alignment {align}, which is not a power of two"
            );
        }
        let Some(end) = addr.checked_add(size) else {
            throw_ub_format!(
                "the allocator handed out {size} bytes at {addr:#x}, past the end of the address space"
            );
        };
        if addr % align != 0 {
            throw_ub_format!(
                "the allocator handed out the block {addr:#x}..{end:#x}, which is not aligned to {align} bytes"
            );
        }
        let Some(toml) = ecx.machine.kmiri_toml.as_ref() else {
            throw_unsup_format!("KMiri: kernel objects need a `[[pool]]` in the KMiri config");
        };
        let paddr_end = paddr + size;
        let in_pool = toml
            .pool_containing(paddr as usize)
            .is_some_and(|pool| paddr_end <= (pool.base_addr + pool.size) as u64);
        if !in_pool {
            throw_ub_format!(
                "the allocator handed out the block {addr:#x}..{end:#x}, which is not inside a pool"
            );
        }
        let global_state = ecx.machine.alloc_addresses.borrow();
        let overlaps =
            |base: u64, len: u64| base < paddr_end.max(paddr + 1) && paddr < base + len.max(1);
        let live = global_state
            .objects
            .range(..paddr_end.max(paddr + 1))
            .next_back()
            .map(|(&base, &(len, _))| (base, len))
            .filter(|&(base, len)| overlaps(base, len));
        let suspended = global_state
            .suspended_objects
            .iter()
            .map(|(&base, &(len, _))| (base, len))
            .find(|&(base, len)| overlaps(base, len));
        if let Some((base, len)) = live.or(suspended) {
            throw_ub_format!(
                "the allocator handed out the block {addr:#x}..{end:#x}, which overlaps the live object at physical address {base:#x}..{:#x}",
                base + len
            );
        }
        interp_ok(())
    }

    /// A fresh, uninitialized allocation for an object of `size` bytes at `paddr`.
    fn new_object_allocation(
        &self,
        paddr: u64,
        size: u64,
        align: u64,
    ) -> Allocation<Provenance, (), MiriAllocBytes> {
        let ecx = self.eval_context_ref();
        let mut allocation = mirch::create_allocation_at(
            paddr as usize,
            Layout::from_size_align(size as usize, align as usize).unwrap(),
            ecx.machine.get_default_alloc_params(),
        );
        allocation.write_uninit(ecx, alloc_range(Size::ZERO, Size::from_bytes(size)));
        allocation
    }

    /// Adds `allocation` as the live object at `paddr` and returns a pointer to it.
    fn insert_object(
        &mut self,
        addr: u64,
        paddr: u64,
        size: u64,
        allocation: Allocation<Provenance, (), MiriAllocBytes>,
    ) -> Pointer {
        let this = self.eval_context_mut();
        let alloc_id =
            this.insert_kernel_allocation(paddr as usize, paddr as usize, allocation, None);
        this.machine.alloc_addresses.borrow_mut().objects.insert(paddr, (size, alloc_id));
        let tag = this.object_root_tag(alloc_id);
        Pointer::new(Some(Provenance::Concrete { alloc_id, tag }), Size::from_bytes(addr))
    }

    /// The root tag of `alloc_id`.
    fn object_root_tag(&self, alloc_id: AllocId) -> BorTag {
        let ecx = self.eval_context_ref();
        match &ecx.machine.borrow_tracker {
            Some(borrow_tracker) =>
                borrow_tracker.borrow_mut().root_ptr_tag(alloc_id, &ecx.machine),
            None => BorTag::default(),
        }
    }

    /// Removes the live object that `ptr` starts, or reports why there is none.
    fn take_live_object(
        &mut self,
        ptr: Pointer,
        what: &str,
    ) -> InterpResult<'tcx, (u64, u64, AllocId)> {
        let this = self.eval_context_mut();
        let addr = ptr.addr().bytes();
        let paddr = this.object_paddr(addr)?;
        let found = this.machine.alloc_addresses.borrow().objects.get(&paddr).copied();
        let Some((size, alloc_id)) = found else {
            if let Some((base, size, _)) = this.live_object_at(paddr) {
                throw_ub_format!(
                    "{what} {addr:#x}, which is inside the object at physical address {base:#x}..{:#x} but not at its start",
                    base + size
                );
            }
            throw_ub_format!(
                "{what} {addr:#x}, which is not a live object: it was already freed, or never allocated"
            );
        };
        if let Some(Provenance::Concrete { alloc_id: ptr_id, .. }) = ptr.provenance
            && ptr_id != alloc_id
        {
            throw_ub_format!(
                "{what} the object at {addr:#x} through a pointer to an earlier object at that address, which was already freed"
            );
        }
        this.machine.alloc_addresses.borrow_mut().objects.remove(&paddr);
        interp_ok((paddr, size, alloc_id))
    }

    /// Ends the allocation of an object, through `ptr` if it points to it.
    fn free_object(&mut self, ptr: Pointer, alloc_id: AllocId, size: u64) -> InterpResult<'tcx> {
        let this = self.eval_context_mut();
        // Free through the pointer the kernel passed, as Miri's own heap shims
        // do; without its provenance, through the root pointer.
        let ptr = match ptr.provenance {
            Some(Provenance::Concrete { .. }) => ptr,
            _ => {
                let tag = this.object_root_tag(alloc_id);
                Pointer::new(Some(Provenance::Concrete { alloc_id, tag }), ptr.addr())
            }
        };
        let align = this.get_alloc_info(alloc_id).align;
        this.deallocate_ptr(
            ptr,
            Some((Size::from_bytes(size), align)),
            MemoryKind::Machine(MiriMemoryKind::Kernel),
        )
    }

    /// Copies the initialization state and provenance of the object `alloc_id`
    /// of `size` bytes at `paddr` onto the pool bytes under it (the bytes
    /// themselves are shared).
    fn show_object_in_pool(
        &mut self,
        paddr: u64,
        size: u64,
        alloc_id: AllocId,
    ) -> InterpResult<'tcx> {
        let this = self.eval_context_mut();
        let Some(pool_id) = this.pool_alloc_at(paddr) else {
            return interp_ok(());
        };
        let base = this.machine.alloc_addresses.borrow().base_paddr[&pool_id];
        let (init, provenance) = {
            let ecx = &*this;
            let object = ecx.get_alloc_raw(alloc_id)?;
            let range = alloc_range(Size::ZERO, Size::from_bytes(size));
            (object.init_mask().prepare_copy(range), object.provenance_prepare_copy(range, ecx))
        };
        let (pool, _) = this.get_alloc_raw_mut(pool_id)?;
        let range = alloc_range(Size::from_bytes(paddr - base), Size::from_bytes(size));
        pool.init_mask_apply_copy(init, range, 1);
        pool.provenance_apply_copy(provenance, range, 1);
        interp_ok(())
    }

    /// Marks `size` pool bytes at `paddr` as uninitialized.
    fn mark_pool_uninit(&mut self, paddr: u64, size: u64) -> InterpResult<'tcx> {
        let this = self.eval_context_mut();
        let Some(pool_id) = this.pool_alloc_at(paddr) else {
            return interp_ok(());
        };
        let base = this.machine.alloc_addresses.borrow().base_paddr[&pool_id];
        let tcx = this.tcx.tcx;
        let (pool, _) = this.get_alloc_raw_mut(pool_id)?;
        pool.write_uninit(
            &tcx,
            alloc_range(Size::from_bytes(paddr - base), Size::from_bytes(size)),
        );
        interp_ok(())
    }

    /// The memory `range` is freed or reused: every thread whose stack lies
    /// there ends, the calls of its frames end as if they returned (their
    /// protectors are released), and its locals are freed. A thread running on
    /// a CPU cannot lose its stack. Such a thread never finishes: joining it is
    /// not supported.
    fn end_threads_on_stack(&mut self, range: Range<u64>) -> InterpResult<'tcx> {
        let this = self.eval_context_mut();
        if range.is_empty() {
            return interp_ok(());
        }
        for id in this.machine.threads.threads_with_stack_in(range.clone()) {
            if this.machine.threads.is_running_on_cpu(id) {
                throw_ub_format!(
                    "the memory {:#x}..{:#x} holds the stack of a thread running on a CPU, and is freed or reused",
                    range.start,
                    range.end
                );
            }
            let (frames, locals) = this.machine.threads.end_thread_without_stack(id);
            this.machine.stackless_threads.insert(id);
            if this.machine.borrow_tracker.is_some() {
                for frame in frames.iter().rev() {
                    this.on_stack_pop(frame)?;
                }
            }
            for (vaddr, alloc_id) in locals {
                if !this.is_alloc_live(alloc_id) {
                    continue;
                }
                let tag = this.object_root_tag(alloc_id);
                this.deallocate_ptr(
                    Pointer::new(
                        Some(Provenance::Concrete { alloc_id, tag }),
                        Size::from_bytes(vaddr),
                    ),
                    None,
                    MemoryKind::Stack,
                )?;
            }
        }
        interp_ok(())
    }
}
