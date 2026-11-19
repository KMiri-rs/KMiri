use std::{fs, io};

use serde::Deserialize;

use crate::KMiriConfigToml;
use crate::mirch::try_kernel_code_vaddr_to_paddr;

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
            cpu_local_size: 0x1_0000,

            // Size and base should align to 0x4000_0000
            kernel_code_base_vaddr: 0xffff_ffff_8000_0000,
            boot_pt_linear_mapping_base_vaddr: 0xffff_8000_0000_0000,
        }
    }

    pub fn new_with_toml(toml: &KMiriConfigToml) -> Self {
        let mut config = Self { total_mem_size: toml.total_mem_size() as usize, ..Self::default() };
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
#[expect(unused)]
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
pub const BUILTIN_LAYOUT_SYMBOLS: [&str; 2] = ["__cpu_local_start", "__cpu_local_end"];

/// Physical addresses of [`BUILTIN_LAYOUT_SYMBOLS`], in the same order.
pub fn builtin_layout_symbol_paddrs() -> [usize; 2] {
    [cpu_local_start_addr(), cpu_local_end_addr()]
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
