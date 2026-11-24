//! A virtio block device on the MMIO transport (VIRTIO 1.3, sections 4.2
//! "Virtio Over MMIO" and 5.2 "Block Device"), at the physical address that
//! `[virtio_blk]` in the KMiri config gives.
//!
//! It has what a driver needs to read and write sectors: the version 2
//! register layout, the feature `VIRTIO_F_VERSION_1` and no device-specific
//! feature, one virtqueue in the split layout (no indirect descriptors, no
//! event index), and the requests `VIRTIO_BLK_T_IN` and `VIRTIO_BLK_T_OUT`
//! (any other completes with `VIRTIO_BLK_S_UNSUPP`). The device processes the
//! available requests when the driver notifies the queue. Every access it makes
//! to the rings and the buffers is a DMA access (`dma.rs`). It does not raise
//! an interrupt: it sets the used-buffer bit of `InterruptStatus`, which a
//! driver can poll, like the used ring. So only a driver that polls for
//! completion works with it; one that waits for the interrupt waits forever.
//! A request is complete when the notifying MMIO write returns.
//!
//! The device accepts the features only if the driver took `VIRTIO_F_VERSION_1`
//! and nothing else (VIRTIO 1.3, 3.1.1: otherwise it leaves `FEATURES_OK`
//! clear), and a request's data must be whole sectors, or it fails with
//! `VIRTIO_BLK_S_IOERR`.
//!
//! The disk holds `capacity_sectors` sectors of 512 bytes. Byte `i` of the
//! disk is initially `i % 251`, so a read can be told from zeroed memory.

use serde::Deserialize;

use super::dma::{device_read, device_write};
use crate::*;

/// The size of the register window: the registers up to 0x100, then the
/// configuration space.
pub const VIRTIO_MMIO_WINDOW: u64 = 0x200;

const MAGIC_VALUE: u32 = 0x7472_6976;
const VERSION: u32 = 2;
const DEVICE_ID_BLOCK: u32 = 2;
/// QEMU's vendor ID, as on QEMU's virtio-mmio devices.
const VENDOR_ID: u32 = 0x554d_4551;
const F_VERSION_1: u64 = 1 << 32;
const STATUS_DRIVER_OK: u32 = 4;
const STATUS_FEATURES_OK: u32 = 8;
const QUEUE_NUM_MAX: u32 = 256;
const DESC_F_NEXT: u16 = 1;
const DESC_F_WRITE: u16 = 2;
const DESC_F_INDIRECT: u16 = 4;
const T_IN: u32 = 0;
const T_OUT: u32 = 1;
const S_OK: u8 = 0;
const S_IOERR: u8 = 1;
const S_UNSUPP: u8 = 2;
const SECTOR_SIZE: u64 = 512;

#[derive(Clone, Debug, Deserialize)]
pub struct VirtioBlkConfigToml {
    /// The physical address of the register window, outside RAM.
    pub mmio_base: u64,
    pub capacity_sectors: u64,
}

pub struct VirtioBlk {
    mmio_base: u64,
    disk: Vec<u8>,
    status: u32,
    device_features_sel: u32,
    driver_features: u64,
    driver_features_sel: u32,
    queue_sel: u32,
    queue: Queue,
    interrupt_status: u32,
}

#[derive(Default)]
struct Queue {
    num: u32,
    ready: bool,
    desc: u64,
    driver: u64,
    device: u64,
    /// The next entry of the available ring to process.
    last_avail: u16,
    /// The index of the next entry of the used ring to fill.
    used_idx: u16,
}

struct Desc {
    addr: u64,
    len: u32,
    flags: u16,
}

impl VirtioBlk {
    pub fn new(config: &VirtioBlkConfigToml) -> Self {
        let size = config.capacity_sectors * SECTOR_SIZE;
        Self {
            mmio_base: config.mmio_base,
            disk: (0..size).map(|i| (i % 251) as u8).collect(),
            status: 0,
            device_features_sel: 0,
            driver_features: 0,
            driver_features_sel: 0,
            queue_sel: 0,
            queue: Queue::default(),
            interrupt_status: 0,
        }
    }

    /// The offset of physical address `paddr` in the register window, if it
    /// lies there.
    pub fn offset_of(&self, paddr: u64) -> Option<u64> {
        let offset = paddr.checked_sub(self.mmio_base)?;
        (offset < VIRTIO_MMIO_WINDOW).then_some(offset)
    }

    fn reset(&mut self) {
        self.status = 0;
        self.device_features_sel = 0;
        self.driver_features = 0;
        self.driver_features_sel = 0;
        self.queue_sel = 0;
        self.queue = Queue::default();
        self.interrupt_status = 0;
    }

    fn config_space(&self) -> [u8; 8] {
        // `struct virtio_blk_config` starts with `le64 capacity`; the fields
        // after it belong to features this device does not offer.
        (self.disk.len() as u64 / SECTOR_SIZE).to_le_bytes()
    }

    pub fn read<'tcx>(&mut self, offset: u64, size: u64) -> InterpResult<'tcx, u64> {
        if offset >= 0x100 {
            let config = self.config_space();
            let start = (offset - 0x100) as usize;
            let mut value = 0u64;
            for i in (0..size as usize).rev() {
                value = value << 8 | u64::from(config.get(start + i).copied().unwrap_or(0));
            }
            return interp_ok(value);
        }
        if size != 4 {
            throw_unsup_format!("virtio-mmio: {size}-byte read of register {offset:#x}");
        }
        let value = match offset {
            0x000 => MAGIC_VALUE,
            0x004 => VERSION,
            0x008 => DEVICE_ID_BLOCK,
            0x00c => VENDOR_ID,
            0x010 => match self.device_features_sel {
                1 => (F_VERSION_1 >> 32) as u32,
                _ => 0,
            },
            0x034 => if self.queue_sel == 0 { QUEUE_NUM_MAX } else { 0 },
            0x044 => u32::from(self.queue_sel == 0 && self.queue.ready),
            0x060 => self.interrupt_status,
            0x070 => self.status,
            // ConfigGeneration: the configuration space never changes.
            0x0fc => 0,
            _ => throw_unsup_format!("virtio-mmio: read of register {offset:#x}"),
        };
        interp_ok(u64::from(value))
    }

    pub fn write<'tcx>(
        &mut self,
        this: &mut MiriInterpCx<'tcx>,
        offset: u64,
        size: u64,
        value: u64,
    ) -> InterpResult<'tcx> {
        if offset >= 0x100 {
            // The configuration fields of this device are read-only.
            return interp_ok(());
        }
        if size != 4 {
            throw_unsup_format!("virtio-mmio: {size}-byte write of register {offset:#x}");
        }
        let value = value as u32;
        let queue0 = self.queue_sel == 0;
        let set_low = |field: &mut u64| *field = *field & !0xffff_ffff | u64::from(value);
        let set_high = |field: &mut u64| *field = *field & 0xffff_ffff | u64::from(value) << 32;
        match offset {
            0x014 => self.device_features_sel = value,
            0x020 => match self.driver_features_sel {
                0 => set_low(&mut self.driver_features),
                1 => set_high(&mut self.driver_features),
                _ => {}
            },
            0x024 => self.driver_features_sel = value,
            0x030 => self.queue_sel = value,
            0x038 if queue0 => {
                if value == 0 || value > QUEUE_NUM_MAX || !value.is_power_of_two() {
                    throw_unsup_format!("virtio-mmio: queue size {value}");
                }
                self.queue.num = value;
            }
            0x044 if queue0 => self.queue.ready = value & 1 != 0,
            0x050 => {
                if value == 0 && self.queue.ready && self.status & STATUS_DRIVER_OK != 0 {
                    self.process_queue(this)?;
                }
            }
            0x064 => self.interrupt_status &= !value,
            0x070 =>
                if value == 0 {
                    self.reset();
                } else {
                    self.set_status(value)?;
                },
            0x080 if queue0 => set_low(&mut self.queue.desc),
            0x084 if queue0 => set_high(&mut self.queue.desc),
            0x090 if queue0 => set_low(&mut self.queue.driver),
            0x094 if queue0 => set_high(&mut self.queue.driver),
            0x0a0 if queue0 => set_low(&mut self.queue.device),
            0x0a4 if queue0 => set_high(&mut self.queue.device),
            // Queue registers of a queue that does not exist.
            0x038 | 0x044 | 0x080 | 0x084 | 0x090 | 0x094 | 0x0a0 | 0x0a4 => {}
            _ => throw_unsup_format!("virtio-mmio: write of register {offset:#x}"),
        }
        interp_ok(())
    }

    fn set_status<'tcx>(&mut self, mut value: u32) -> InterpResult<'tcx> {
        if value & STATUS_FEATURES_OK != 0 && self.status & STATUS_FEATURES_OK == 0 {
            let offered = F_VERSION_1;
            if self.driver_features & !offered != 0 || self.driver_features & F_VERSION_1 == 0 {
                value &= !STATUS_FEATURES_OK;
            }
        }
        if value & STATUS_DRIVER_OK != 0 && value & STATUS_FEATURES_OK == 0 {
            throw_unsup_format!("virtio-mmio: DRIVER_OK set without FEATURES_OK");
        }
        self.status = value;
        interp_ok(())
    }

    /// Completes every request the driver has made available.
    fn process_queue<'tcx>(&mut self, this: &mut MiriInterpCx<'tcx>) -> InterpResult<'tcx> {
        let num = self.queue.num;
        if num == 0 {
            throw_unsup_format!("virtio-mmio: queue notified before its size was set");
        }
        loop {
            let avail_idx = read_u16(this, self.queue.driver + 2)?;
            if avail_idx == self.queue.last_avail {
                break;
            }
            let slot = u64::from(u32::from(self.queue.last_avail) % num);
            let head = read_u16(this, self.queue.driver + 4 + 2 * slot)?;
            let written = self.handle_request(this, head)?;
            let used = self.queue.device;
            let slot = u64::from(u32::from(self.queue.used_idx) % num);
            let mut elem = u32::from(head).to_le_bytes().to_vec();
            elem.extend(written.to_le_bytes());
            device_write(this, used + 4 + 8 * slot, &elem)?;
            self.queue.used_idx = self.queue.used_idx.wrapping_add(1);
            device_write(this, used + 2, &self.queue.used_idx.to_le_bytes())?;
            self.queue.last_avail = self.queue.last_avail.wrapping_add(1);
            self.interrupt_status |= 1;
        }
        interp_ok(())
    }

    /// Performs the request whose descriptor chain starts at `head`, and
    /// returns the number of bytes written to its device-writable buffers.
    fn handle_request<'tcx>(&mut self, this: &mut MiriInterpCx<'tcx>, head: u16) -> InterpResult<'tcx, u32> {
        let num = self.queue.num;
        let mut chain = Vec::new();
        let mut index = head;
        loop {
            if u32::from(index) >= num || chain.len() as u32 == num {
                throw_unsup_format!("virtio-mmio: malformed descriptor chain at {head}");
            }
            let raw = device_read(this, self.queue.desc + 16 * u64::from(index), 16)?;
            let desc = Desc {
                addr: u64::from_le_bytes(raw[0..8].try_into().unwrap()),
                len: u32::from_le_bytes(raw[8..12].try_into().unwrap()),
                flags: u16::from_le_bytes(raw[12..14].try_into().unwrap()),
            };
            if desc.flags & DESC_F_INDIRECT != 0 {
                throw_unsup_format!("virtio-mmio: indirect descriptors are not offered");
            }
            let next = u16::from_le_bytes(raw[14..16].try_into().unwrap());
            let has_next = desc.flags & DESC_F_NEXT != 0;
            chain.push(desc);
            if !has_next {
                break;
            }
            index = next;
        }
        let split = chain.iter().position(|d| d.flags & DESC_F_WRITE != 0).unwrap_or(chain.len());
        let (readable, writable) = chain.split_at(split);
        if writable.iter().any(|d| d.flags & DESC_F_WRITE == 0) {
            throw_unsup_format!("virtio-mmio: a device-readable descriptor follows a writable one");
        }
        let mut out = Vec::new();
        for desc in readable {
            out.extend(device_read(this, desc.addr, desc.len as usize)?);
        }
        let writable_len: u64 = writable.iter().map(|d| u64::from(d.len)).sum();
        if out.len() < 16 || writable_len == 0 {
            throw_unsup_format!("virtio-mmio: a block request without a header or status byte");
        }
        let typ = u32::from_le_bytes(out[0..4].try_into().unwrap());
        let sector = u64::from_le_bytes(out[8..16].try_into().unwrap());
        let data_len = writable_len - 1;
        let start = sector.checked_mul(SECTOR_SIZE);
        let in_disk = |len: u64| {
            len.is_multiple_of(SECTOR_SIZE)
                && start
                    .and_then(|s| s.checked_add(len))
                    .is_some_and(|end| end <= self.disk.len() as u64)
        };
        let mut reply = Vec::new();
        let status = match typ {
            T_IN if in_disk(data_len) => {
                let s = start.unwrap() as usize;
                reply.extend_from_slice(&self.disk[s..s + data_len as usize]);
                S_OK
            }
            T_OUT if in_disk(out.len() as u64 - 16) => {
                let s = start.unwrap() as usize;
                self.disk[s..s + out.len() - 16].copy_from_slice(&out[16..]);
                S_OK
            }
            T_IN | T_OUT => S_IOERR,
            _ => S_UNSUPP,
        };
        // The data goes to the device-writable buffers before the status byte,
        // which is the last of them; on an error only the status is written.
        let status_at = writable_len - 1;
        let mut pos = 0u64;
        for desc in writable {
            let len = u64::from(desc.len);
            let data_end = (reply.len() as u64).min(pos + len);
            if data_end > pos {
                device_write(this, desc.addr, &reply[pos as usize..data_end as usize])?;
            }
            if (pos..pos + len).contains(&status_at) {
                device_write(this, desc.addr + (status_at - pos), &[status])?;
            }
            pos += len;
        }
        interp_ok(if status == S_OK && typ == T_IN { writable_len as u32 } else { 1 })
    }
}

fn read_u16<'tcx>(this: &mut MiriInterpCx<'tcx>, paddr: u64) -> InterpResult<'tcx, u16> {
    let bytes = device_read(this, paddr, 2)?;
    interp_ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}
