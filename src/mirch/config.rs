use std::{fs, io};

use serde::Deserialize;

use super::{MemoryMapEntry, MemoryMapKind};
use crate::KMiriConfigToml;
use crate::mirch::{config_cpu_local_size, try_kernel_code_vaddr_to_paddr};

/// Parses a JSON configuration file into a `PhysConfig` structure.
pub fn parse_json_file(file_path: &str) -> Result<PhysConfig, io::Error> {
    let file_content = fs::read_to_string(file_path)?;

    let config: PhysConfig = serde_json::from_str(&file_content)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    assert!(config.total_mem_size > 0, "total_mem_size must be greater than 0");
    assert!(
        config.total_mem_size > config.kernel_code_size,
        "total_mem_size must be greater than kernel code size"
    );
    assert!(
        config.kernel_code_size
            > kernel_static_start_addr() + config.kernel_static_size + config.cpu_local_size,
        "kernel_code_size need to be bigger enough"
    );

    const INIT_MAPPING_SIZE: usize = 0x4000_0000; // 1GB
    assert!(
        config.boot_pt_linear_mapping_base_vaddr.is_multiple_of(INIT_MAPPING_SIZE),
        "boot_pt_linear_mapping_base_vaddr must be aligned to {INIT_MAPPING_SIZE}"
    );
    assert!(
        config.kernel_code_base_vaddr.is_multiple_of(INIT_MAPPING_SIZE),
        "kernel_code_base_vaddr must be aligned to ${INIT_MAPPING_SIZE}"
    );
    Ok(config)
}

/// Physical Memory Configuration.
///
/// Represents the layout of physical memory in the system with the following structure:
///
/// |<-------------------------------Kernel Code Section-------------------------------->|
/// |<-Boot PT->|<-Kernel Static Section->|<-CPU-local Section->|<-Kernel Stack Section->|
/// |-----------|------------------------------------------------------------------------|
/// 0x0      0x10000                                                             `kernel_code_size`
///
/// |<-Kernel Code Section->|<-----Free Pages------>|
/// |-----------------------|-----------------------|
/// 0x0              `kernel_code_size`        `total_mem_size`
///
#[derive(Deserialize, Clone, Copy, Debug)]
pub struct PhysConfig {
    /// Total physical memory size in bytes
    pub total_mem_size: usize,

    /// Size of kernel code section in bytes (includes boot page table, static data, etc.)
    pub kernel_code_size: usize,

    /// Size of kernel static data section in bytes (after boot page table)
    pub kernel_static_size: usize,

    /// Size per-CPU local storage area in bytes
    pub cpu_local_size: usize,

    // Paging-related configs
    /// Base virtual address for kernel code mapping
    pub kernel_code_base_vaddr: usize,

    /// Base virtual address for boot page table linear mapping
    pub boot_pt_linear_mapping_base_vaddr: usize,
}

impl PhysConfig {
    /// Creates a zero-initialized physical memory configuration.
    pub const fn zero() -> Self {
        Self {
            total_mem_size: 0,
            kernel_code_size: 0,
            kernel_static_size: 0,
            cpu_local_size: 0,

            kernel_code_base_vaddr: 0,
            boot_pt_linear_mapping_base_vaddr: 0,
        }
    }

    /// Creates a default physical memory configuration with typical values:
    ///
    /// |<-------------------------------Kernel Code Section-------------------------------->|
    /// |<-Boot PT->|<-Kernel Static Section->|<-CPU-local Section->|<-Kernel Stack Section->|
    /// |-----------|-------------------------|---------------------|------------------------|
    /// 0x0      0x1_0000                 0x40_0000             0x41_0000                0x100_0000
    ///
    /// |<-Kernel Code Section->|<-----Free Pages------>|
    /// |-----------------------|-----------------------|
    /// 0x0                 0x100_0000             0x800_0000
    pub const fn default() -> Self {
        Self {
            total_mem_size: 0x800_0000,
            kernel_code_size: 0x100_0000,
            kernel_static_size: 0x3f_0000,
            cpu_local_size: config_cpu_local_size() as usize,

            // Size and base should align to 0x4000_0000
            kernel_code_base_vaddr: 0xffff_ffff_8000_0000,
            boot_pt_linear_mapping_base_vaddr: 0xffff_8000_0000_0000,
        }
    }

    /// The size of the kernel code section for a CPU-local section of `cpu_local_size`
    /// bytes. The kernel stack section, right after the CPU-local section, keeps its size.
    pub fn kernel_code_size_for(cpu_local_size: usize) -> usize {
        let default = Self::default();
        default.kernel_code_size - default.cpu_local_size + cpu_local_size
    }

    pub fn new_with_toml(toml: &KMiriConfigToml) -> Self {
        let cpu_local_size = toml.cpu_local_size() as usize;
        let mut config = Self {
            total_mem_size: toml.total_mem_size() as usize,
            kernel_code_size: Self::kernel_code_size_for(cpu_local_size),
            cpu_local_size,
            ..Self::default()
        };
        if !toml.page_table_enabled() {
            // Allocation lookup uses physical addresses without page tables. Give
            // interpreter-created Rust allocations matching identity addresses too.
            config.kernel_code_base_vaddr = 0;
        }
        config
    }
}

/// Physical code section for kernel.
#[derive(Debug, PartialEq, PartialOrd, Eq, Ord)]
pub enum CodeSection {
    BootPt,
    Static,
    CpuLocal,
    Stack,
}

impl CodeSection {
    pub fn paddr(paddr: u64) -> Option<Self> {
        let paddr = paddr as usize;
        Some(if paddr < kernel_static_start_addr() {
            Self::BootPt
        } else if paddr < cpu_local_start_addr() {
            Self::Static
        } else if paddr < kernel_stack_start_addr() {
            Self::CpuLocal
        } else if paddr <= kernel_stack_end_addr() {
            Self::Stack
        } else {
            return None;
        })
    }

    /// NOTE: this can't be called for boot_pt addr, because its base vaddr differs
    /// from other section base addr.
    #[expect(unused)]
    pub fn vaddr(vaddr: u64) -> Option<Self> {
        let paddr = try_kernel_code_vaddr_to_paddr(vaddr as usize)?;
        Self::paddr(paddr as u64)
    }
}

static mut PHYSICAL_MEM_CONFIG: PhysConfig = PhysConfig::zero();

fn config<'a>() -> &'a PhysConfig {
    #[allow(clippy::deref_addrof)]
    unsafe {
        &*(&raw const PHYSICAL_MEM_CONFIG)
    }
}

/// Initializes the global physical memory configuration
pub(super) fn init(config: PhysConfig) {
    unsafe { PHYSICAL_MEM_CONFIG = config };
}

/// Returns the system page size (4KB)
pub const fn page_size() -> usize {
    0x1000 // 4KB
}

/// Returns total physical memory size in bytes
pub fn total_mem_size() -> usize {
    config().total_mem_size
}

/// Returns total number of physical memory pages
pub fn total_page_num() -> usize {
    total_mem_size() / page_size()
}

// Kernel code section accessors
/// Returns starting physical address of kernel code (always 0x0)
pub const fn kernel_code_start() -> usize {
    0
}

/// Returns ending physical address of kernel code section
pub fn kernel_code_end() -> usize {
    config().kernel_code_size
}

/// Returns number of pages in kernel code section
#[expect(unused)]
pub fn kernel_code_page_num() -> usize {
    config().kernel_code_size / page_size()
}

// Kernel static section accessors
/// Returns starting physical address of kernel static data (after 64KB boot page table)
pub const fn kernel_static_start_addr() -> usize {
    0x10000
}

/// Returns ending physical address of kernel static data
pub fn kernel_static_end_addr() -> usize {
    kernel_static_start_addr() + config().kernel_static_size
}

// CPU-local storage accessors
/// Returns size of per-CPU local storage area
pub fn cpu_local_segment_size() -> usize {
    config().cpu_local_size
}

/// Returns starting physical address of CPU-local storage
pub fn cpu_local_start_addr() -> usize {
    kernel_static_end_addr()
}

/// Returns ending physical address of CPU-local storage
pub fn cpu_local_end_addr() -> usize {
    cpu_local_start_addr() + cpu_local_segment_size()
}

/// Linker-script symbols that KMiri provides itself, because KMiri lays out the
/// sections they bound. `[layout]` entries in `kmiri.toml` take precedence.
pub const BUILTIN_LAYOUT_SYMBOLS: [&str; 4] =
    ["__cpu_local_start", "__cpu_local_end", "__kernel_start", "__kernel_end"];

/// Physical addresses of [`BUILTIN_LAYOUT_SYMBOLS`], in the same order.
pub fn builtin_layout_symbol_paddrs() -> [usize; 4] {
    [cpu_local_start_addr(), cpu_local_end_addr(), kernel_code_start(), kernel_code_end()]
}

// Kernel stack accessors
/// Returns starting physical address of kernel stacks
pub fn kernel_stack_start_addr() -> usize {
    cpu_local_end_addr()
}

/// Returns ending physical address of kernel stacks
pub fn kernel_stack_end_addr() -> usize {
    kernel_code_end()
}

// Virtual address accessors
/// Returns base virtual address for kernel code mapping
pub fn kernel_code_base_vaddr() -> usize {
    config().kernel_code_base_vaddr
}

/// Returns base virtual address for boot page table linear mapping
pub fn boot_pt_linear_mapping_base_vaddr() -> usize {
    config().boot_pt_linear_mapping_base_vaddr
}

/// Builds the default firmware memory map for a machine with `total_mem_size` bytes
/// and a kernel image occupying `[0, kernel_code_size)`.
///
/// Mirrors the shape of a native x86 firmware map (low usable RAM, an absent
/// non-RAM hole of 96 pages / 384 KiB like `0xa0000..0x100000`, a firmware-reserved
/// region, and high usable RAM above the hole) placed above `kernel_code_size` so
/// it does not overlap KMiri's kernel code/static/stack layout.
pub fn default_memory_map(total_mem_size: usize, kernel_code_size: usize) -> Vec<MemoryMapEntry> {
    let page = page_size();
    let hole_len = 0x6_0000usize; // 384 KiB (96 pages), same size as native 0xa0000..0x100000
    let reserved_len = 0x2_0000usize; // 128 KiB (32 pages)
    let min_headroom = 0x20_0000usize; // 2 MiB above kernel image
    if total_mem_size >= kernel_code_size.saturating_add(min_headroom) {
        let avail = total_mem_size - kernel_code_size;
        let hole_base = (kernel_code_size + avail * 3 / 4) & !(page - 1);
        let reserved_base = hole_base + hole_len;
        let high_ram_base = reserved_base + reserved_len;
        if high_ram_base < total_mem_size {
            return vec![
                MemoryMapEntry { base: 0, len: hole_base as u64, kind: MemoryMapKind::Usable },
                MemoryMapEntry {
                    base: reserved_base as u64,
                    len: reserved_len as u64,
                    kind: MemoryMapKind::Reserved,
                },
                MemoryMapEntry {
                    base: high_ram_base as u64,
                    len: (total_mem_size - high_ram_base) as u64,
                    kind: MemoryMapKind::Usable,
                },
            ];
        }
    }
    vec![MemoryMapEntry { base: 0, len: total_mem_size as u64, kind: MemoryMapKind::Usable }]
}

/// Resolves the active firmware memory map from `kmiri.toml` (if configured) or the default map.
pub fn resolve_memory_map(
    toml: Option<&KMiriConfigToml>,
    config: &PhysConfig,
) -> Vec<MemoryMapEntry> {
    if let Some(toml) = toml
        && !toml.memory_map().is_empty()
    {
        let mut entries = toml.memory_map().to_vec();
        entries.sort_by_key(|e| (e.base, e.len));
        return entries;
    }
    default_memory_map(config.total_mem_size, config.kernel_code_size)
}
