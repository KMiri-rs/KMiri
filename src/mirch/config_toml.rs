use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;

use super::config::{PhysConfig, page_size};
use super::virtio_blk::{VIRTIO_MMIO_WINDOW, VirtioBlkConfigToml};

#[derive(Clone, Debug, Deserialize)]
pub struct KMiriConfigToml {
    #[serde(default = "config_page_table")]
    page_table: bool,
    /// The upper limit of physical memory for the kernel.
    total_mem_size: u64,
    /// The upper limit of a spawned non-main thread.
    #[serde(default = "config_stack_mem_size")]
    stack_mem_size: u64,
    /// The size of the CPU-local section, in bytes: a multiple of the page size.
    /// The kernel code section grows with it, so the kernel stack section keeps its size.
    #[serde(default)]
    cpu_local_size: Option<u64>,
    /// Linker-script / `extern` symbols that should keep a fixed address.
    /// Unknown `extern fn` symbols are rejected unless listed here (KMiri#76).
    /// The value is a physical address.
    #[serde(default)]
    layout: BTreeMap<String, u64>,
    /// Link sections that a linker script collects into one array between a
    /// start and an end symbol (`MiriMachine::init_link_section_arrays`).
    #[serde(default)]
    link_section: Vec<LinkSectionToml>,
    #[serde(default)]
    kalloc: Vec<KAlloc>,
    /// Symbols defined in user code that should take precedence over built-in shims.
    /// This is useful when you have your own implementation of standard library functions
    /// (like `strlen`, `memcpy`, etc.) and want Miri to use them instead of reporting
    /// a symbol clash error as in <https://github.com/KMiri-rs/KMiri/issues/171>.
    ///
    /// Example:
    /// ```toml
    /// [user_preferred_symbol]
    /// strlen = true
    /// memcpy = true
    /// ```
    #[serde(default)]
    user_preferred_symbol: BTreeMap<String, bool>,
    /// Configuration for KMiri's optional interrupt model.
    #[serde(default)]
    irq: IrqConfigToml,
    /// Configuration for the simulated CPUs.
    #[serde(default)]
    smp: SmpConfigToml,
    /// A virtio block device on the MMIO transport (`virtio_blk.rs`). Without
    /// it, nothing is outside RAM.
    #[serde(default)]
    virtio_blk: Option<VirtioBlkConfigToml>,
    /// Optional firmware memory map entries (`[[memory_map]]`).
    /// Ranges omitted below `total_mem_size` are non-RAM holes.
    /// When omitted, KMiri uses a default map with a firmware hole and a reserved
    /// region above the kernel image when `total_mem_size` is large enough.
    #[serde(default)]
    memory_map: Vec<MemoryMapEntry>,
}

/// Kind of a firmware memory map entry in `kmiri.toml`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryMapKind {
    /// Usable physical RAM.
    Usable,
    /// Firmware-reserved non-RAM region.
    Reserved,
}

impl MemoryMapKind {
    /// ABI code returned by `kern_miri_memory_map_entry`.
    pub const fn as_u32(self) -> u32 {
        match self {
            Self::Usable => 1,
            Self::Reserved => 2,
        }
    }
}

/// One entry in the machine's firmware memory map (`[[memory_map]]` in `kmiri.toml`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub struct MemoryMapEntry {
    pub base: u64,
    pub len: u64,
    pub kind: MemoryMapKind,
}

/// Configuration for the simulated CPUs.
///
/// CPU 0 runs the program's entry point. Every other CPU stays off until the
/// program starts it with `kern_miri_start_cpu`.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct SmpConfigToml {
    num_cpus: usize,
    /// Probability that, at the end of a basic block, execution moves to another
    /// running CPU chosen uniformly at random.
    switch_rate: f64,
    /// Seed of the random choices about which CPU runs next.
    seed: u64,
}

impl Default for SmpConfigToml {
    fn default() -> Self {
        Self { num_cpus: 1, switch_rate: 0.01, seed: 0 }
    }
}

impl SmpConfigToml {
    pub fn num_cpus(&self) -> usize {
        self.num_cpus
    }

    pub fn switch_rate(&self) -> f64 {
        self.switch_rate
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }
}

/// Configuration for deterministic and randomized interrupt injection.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct IrqConfigToml {
    enabled: bool,
    tick_interval_basic_blocks: u64,
    inject_at_basic_blocks: Vec<u64>,
    random_injection_rate: f64,
    random_seed: u64,
    trace: bool,
}

impl Default for IrqConfigToml {
    fn default() -> Self {
        Self {
            enabled: false,
            tick_interval_basic_blocks: 0,
            inject_at_basic_blocks: Vec::new(),
            random_injection_rate: 0.0,
            random_seed: 0,
            trace: false,
        }
    }
}

impl IrqConfigToml {
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn tick_interval_basic_blocks(&self) -> u64 {
        self.tick_interval_basic_blocks
    }

    pub fn inject_at_basic_blocks(&self) -> &[u64] {
        &self.inject_at_basic_blocks
    }

    pub fn random_injection_rate(&self) -> f64 {
        self.random_injection_rate
    }

    pub fn random_seed(&self) -> u64 {
        self.random_seed
    }

    pub fn trace(&self) -> bool {
        self.trace
    }
}

impl KMiriConfigToml {
    pub fn new(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path)
            .map_err(|err| format!("failed to read `{}`: {err}", path.display()))?;
        let config: Self = basic_toml::from_str(&text)
            .map_err(|err| format!("failed to parse `{}`: {err}", path.display()))?;
        let rate = config.irq.random_injection_rate;
        if !rate.is_finite() || !(0.0..=1.0).contains(&rate) {
            return Err(format!(
                "invalid `[irq].random_injection_rate` in `{}`: expected a value in 0.0..=1.0",
                path.display()
            ));
        }
        let rate = config.smp.switch_rate;
        if !rate.is_finite() || !(0.0..=1.0).contains(&rate) {
            return Err(format!(
                "invalid `[smp].switch_rate` in `{}`: expected a value in 0.0..=1.0",
                path.display()
            ));
        }
        if config.smp.num_cpus == 0 {
            return Err(format!(
                "invalid `[smp].num_cpus` in `{}`: expected at least 1",
                path.display()
            ));
        }
        let size = config.cpu_local_size();
        if size == 0 || !size.is_multiple_of(page_size() as u64) {
            return Err(format!(
                "invalid `cpu_local_size` in `{}`: expected a non-zero multiple of {:#x}",
                path.display(),
                page_size()
            ));
        }
        if size != config_cpu_local_size()
            && (size >= config.total_mem_size
                || PhysConfig::kernel_code_size_for(size as usize) as u64 >= config.total_mem_size)
        {
            return Err(format!(
                "invalid `cpu_local_size` in `{}`: the kernel code section would not fit in `total_mem_size`",
                path.display()
            ));
        }
        if let Some(blk) = &config.virtio_blk
            && (!config.page_table
                || blk.mmio_base < config.total_mem_size
                || !blk.mmio_base.is_multiple_of(VIRTIO_MMIO_WINDOW)
                || blk.capacity_sectors.checked_mul(512).is_none_or(|size| size > config.total_mem_size))
        {
            return Err(format!(
                "invalid `[virtio_blk]` in `{}`: the device needs `page_table = true`, `mmio_base` must be outside RAM and aligned to {VIRTIO_MMIO_WINDOW:#x}, and the disk no larger than RAM",
                path.display()
            ));
        }
        if !config.memory_map.is_empty() {
            let page = page_size() as u64;
            let mut sorted = config.memory_map.clone();
            sorted.sort_by_key(|e| (e.base, e.len));
            let mut prev_end = 0u64;
            for entry in &sorted {
                if entry.len == 0
                    || !entry.base.is_multiple_of(page)
                    || !entry.len.is_multiple_of(page)
                {
                    return Err(format!(
                        "invalid `[[memory_map]]` in `{}`: base ({:#x}) and len ({:#x}) must be non-zero multiples of {page:#x}",
                        path.display(),
                        entry.base,
                        entry.len
                    ));
                }
                let Some(end) = entry.base.checked_add(entry.len) else {
                    return Err(format!(
                        "invalid `[[memory_map]]` in `{}`: entry at {:#x} overflows",
                        path.display(),
                        entry.base
                    ));
                };
                if end > config.total_mem_size {
                    return Err(format!(
                        "invalid `[[memory_map]]` in `{}`: entry [{:#x}, {:#x}) exceeds `total_mem_size` ({:#x})",
                        path.display(),
                        entry.base,
                        end,
                        config.total_mem_size
                    ));
                }
                if entry.base < prev_end {
                    return Err(format!(
                        "invalid `[[memory_map]]` in `{}`: overlapping entries at {:#x}",
                        path.display(),
                        entry.base
                    ));
                }
                prev_end = end;
            }
            let kernel_end =
                PhysConfig::kernel_code_size_for(config.cpu_local_size() as usize) as u64;
            let mut covered = 0u64;
            for entry in &sorted {
                if entry.kind != MemoryMapKind::Usable {
                    continue;
                }
                if entry.base <= covered && entry.base + entry.len > covered {
                    covered = entry.base + entry.len;
                }
            }
            if covered < kernel_end {
                return Err(format!(
                    "invalid `[[memory_map]]` in `{}`: usable RAM must cover the kernel image [0x0, {kernel_end:#x})",
                    path.display()
                ));
            }
        }
        Ok(config)
    }

    pub fn memory_map(&self) -> &[MemoryMapEntry] {
        &self.memory_map
    }

    pub fn total_mem_size(&self) -> u64 {
        self.total_mem_size
    }

    pub fn stack_mem_size(&self) -> u64 {
        self.stack_mem_size
    }

    pub fn cpu_local_size(&self) -> u64 {
        self.cpu_local_size.unwrap_or_else(config_cpu_local_size)
    }

    pub fn cpu_local_size_explicit(&self) -> bool {
        self.cpu_local_size.is_some()
    }

    pub fn page_table_enabled(&self) -> bool {
        self.page_table
    }

    pub fn symbol_addr(&self, symbol: &str) -> Option<u64> {
        self.layout.get(symbol).copied()
    }

    pub fn layout_symbols(&self) -> impl Iterator<Item = (&str, u64)> {
        self.layout.iter().map(|(name, addr)| (name.as_str(), *addr))
    }

    pub fn link_sections(&self) -> &[LinkSectionToml] {
        &self.link_section
    }

    pub fn get_kalloc(&self, base_addr: usize) -> Option<&KAlloc> {
        self.kalloc.iter().find(|kalloc| kalloc.base_addr == base_addr)
    }

    pub fn prefers_user_implementation(&self, symbol_name: &str) -> bool {
        self.user_preferred_symbol.get(symbol_name).copied().unwrap_or(false)
    }

    pub fn irq(&self) -> &IrqConfigToml {
        &self.irq
    }

    pub fn smp(&self) -> &SmpConfigToml {
        &self.smp
    }

    pub fn virtio_blk(&self) -> Option<&VirtioBlkConfigToml> {
        self.virtio_blk.as_ref()
    }
}

/// Enable page table by default.
/// FIXME: default to false if asterinas migrates to toml config.
fn config_page_table() -> bool {
    true
}

pub fn config_stack_mem_size() -> u64 {
    0x4000 // 16 KB
}

pub const fn config_cpu_local_size() -> u64 {
    0x1_0000 // 64 KB
}

/// A link section laid out as one array: `start` names its first element and
/// `end` the address just past its last one.
#[derive(Clone, Debug, Deserialize)]
pub struct LinkSectionToml {
    pub name: String,
    pub start: String,
    pub end: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct KAlloc {
    pub name: String,
    pub base_addr: usize,
    pub size: usize,
    pub align: usize,
}

#[test]
fn layout() {
    let config: KMiriConfigToml = basic_toml::from_str(
        "
[layout]
_stext   = 0x80000000
_etext   = 0x8001b600
_sflash  = 0x80000000
_eflash  = 0x80200000
_sapps   = 0x80100000
_eapps   = 0x80200000
_ssram   = 0x80200000
_esram   = 0x80400000
_sappmem = 0x8021b680
_eappmem = 0x80400000
",
    )
    .unwrap();
    assert_eq!(config.layout["_stext"], 0x80000000);
    assert_eq!(config.layout["_sapps"], 0x80100000);
    assert_eq!(config.layout["_eappmem"], 0x80400000);
}

#[test]
fn irq_defaults_and_values() {
    let config: KMiriConfigToml = basic_toml::from_str("total_mem_size = 0x8000000\n").unwrap();
    assert!(!config.irq().enabled());

    let config: KMiriConfigToml = basic_toml::from_str(
        r#"
total_mem_size = 0x8000000

[irq]
enabled = true
tick_interval_basic_blocks = 8
inject_at_basic_blocks = [12, 20]
random_injection_rate = 0.25
random_seed = 7
trace = true
"#,
    )
    .unwrap();
    assert!(config.irq().enabled());
    assert_eq!(config.irq().tick_interval_basic_blocks(), 8);
    assert_eq!(config.irq().inject_at_basic_blocks(), [12, 20]);
    assert_eq!(config.irq().random_injection_rate(), 0.25);
    assert_eq!(config.irq().random_seed(), 7);
    assert!(config.irq().trace());
}
