//! Accesses that a device makes to RAM by physical address (DMA).
//!
//! A device reaches memory by physical address, not through a Rust pointer.
//! Whatever lies there is accessed as through a pointer cast from that address,
//! with all the checks of such an access: an interpreter allocation (a static,
//! a heap object, a stack) under its mutability, bounds, aliasing model and
//! data race detector, and untyped pages as raw bytes (`kernel_copy`). So a
//! device write to an immutable static is reported as a write to read-only
//! memory, as a CPU write would be.
//!
//! The device needs no pointer to what it touches, so each allocation it
//! touches is first exposed (with its root tag), as Miri does for memory
//! passed to native code, and stays exposed.
//!
//! The access is made on the CPU thread whose MMIO access made the device act:
//! KMiri has no separate device agent. So another CPU that reads what the
//! device wrote to an interpreter allocation must synchronize with that CPU,
//! or the read is reported as a data race; on hardware it would synchronize
//! with the device's completion instead. Untyped pages are not checked for
//! data races, so rings and buffers there are not affected.

use rustc_abi::{Align, Size};
use rustc_middle::mir::interpret::{AllocInit, alloc_range};

use super::config::{boot_pt_linear_mapping_base_vaddr, total_mem_size};
use super::physical_mem::kernel_copy;
use crate::*;

/// Writes `bytes` to RAM at physical address `paddr`, as a device does.
pub fn device_write<'tcx>(
    this: &mut MiriInterpCx<'tcx>,
    paddr: u64,
    bytes: &[u8],
) -> InterpResult<'tcx> {
    let kind = MiriMemoryKind::Machine.into();
    let mut done = 0;
    for (start, len) in pieces(this, paddr, bytes.len())? {
        let size = Size::from_bytes(len as u64);
        let tmp: Pointer = this.allocate_ptr(size, Align::ONE, kind, AllocInit::Uninit)?.into();
        this.write_bytes_ptr(tmp, bytes[done..done + len].iter().copied())?;
        kernel_copy(this, device_ptr(start), tmp, len)?;
        this.deallocate_ptr(tmp, None, kind)?;
        done += len;
    }
    interp_ok(())
}

/// Reads `len` bytes of RAM at physical address `paddr`, as a device does.
///
/// Uninitialized bytes are not supported: the device would see arbitrary
/// values, which KMiri cannot give it.
pub fn device_read<'tcx>(
    this: &mut MiriInterpCx<'tcx>,
    paddr: u64,
    len: usize,
) -> InterpResult<'tcx, Vec<u8>> {
    let kind = MiriMemoryKind::Machine.into();
    let pieces = pieces(this, paddr, len)?;
    let mut out = Vec::with_capacity(len);
    for (start, len) in pieces {
        let size = Size::from_bytes(len as u64);
        let tmp: Pointer = this.allocate_ptr(size, Align::ONE, kind, AllocInit::Uninit)?.into();
        kernel_copy(this, tmp, device_ptr(start), len)?;
        let (alloc_id, _, _) = this.ptr_get_alloc_id(tmp, 0)?;
        let alloc = this.get_alloc_raw(alloc_id)?;
        let range = alloc_range(Size::ZERO, size);
        let init = alloc.init_mask().is_range_initialized(range).is_ok();
        let bytes = alloc.get_bytes_unchecked(range).to_vec();
        this.deallocate_ptr(tmp, None, kind)?;
        if !init {
            throw_unsup_format!(
                "a device read uninitialized memory in {len} bytes at physical address {start:#x}"
            );
        }
        out.extend(bytes);
    }
    interp_ok(out)
}

/// The kernel's linear mapping of `paddr`, as an address with no provenance:
/// the access finds whatever is there, as an access through a pointer cast
/// from that address does.
fn device_ptr(paddr: u64) -> Pointer {
    let vaddr = boot_pt_linear_mapping_base_vaddr() as u64 + paddr;
    Pointer::new(Some(Provenance::Wildcard), Size::from_bytes(vaddr))
}

/// Splits the `len` bytes at `paddr` at the bounds of the interpreter
/// allocations there (each piece is one allocation or lies outside all of
/// them), after exposing those allocations.
fn pieces<'tcx>(
    this: &MiriInterpCx<'tcx>,
    paddr: u64,
    len: usize,
) -> InterpResult<'tcx, Vec<(u64, usize)>> {
    let Some(end) = paddr.checked_add(len as u64).filter(|&end| end <= total_mem_size() as u64)
    else {
        throw_unsup_format!(
            "a device accessed {len} bytes at physical address {paddr:#x}, outside RAM"
        );
    };
    let candidates: Vec<(u64, AllocId)> = {
        let state = this.machine.alloc_addresses.borrow();
        let map = &state.int_to_ptr_map;
        let first = map.partition_point(|(base, _)| *base < paddr);
        let last = map.partition_point(|(base, _)| *base < end);
        map[first.saturating_sub(1)..last].to_vec()
    };
    let mut cuts = vec![paddr, end];
    for (base, alloc_id) in candidates {
        let size = this.get_alloc_info(alloc_id).size.bytes();
        if size == 0 || base + size <= paddr {
            continue;
        }
        crate::alloc_addresses::expose_allocation(paddr as usize, alloc_id, this);
        cuts.extend([base, base + size].into_iter().filter(|&cut| paddr < cut && cut < end));
    }
    cuts.sort_unstable();
    cuts.dedup();
    interp_ok(cuts.windows(2).map(|w| (w[0], (w[1] - w[0]) as usize)).collect())
}
