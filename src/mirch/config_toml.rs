use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct KMiriConfigToml {
    #[serde(default = "config_page_table")]
    page_table: bool,
    /// The upper limit of physical memory for the kernel.
    total_mem_size: u64,
    /// The upper limit of a spawned non-main thread.
    #[serde(default = "config_stack_mem_size")]
    stack_mem_size: u64,
    /// Linker-script / `extern` symbols that should keep a fixed address.
    /// Unknown `extern fn` symbols are rejected unless listed here (KMiri#76).
    /// The value is a physical address.
    #[serde(default)]
    layout: BTreeMap<String, u64>,
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
        Ok(config)
    }

    pub fn total_mem_size(&self) -> u64 {
        self.total_mem_size
    }

    pub fn stack_mem_size(&self) -> u64 {
        self.stack_mem_size
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
}

/// Enable page table by default.
/// FIXME: default to false if asterinas migrates to toml config.
fn config_page_table() -> bool {
    true
}

pub fn config_stack_mem_size() -> u64 {
    0x4000 // 16 KB
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
