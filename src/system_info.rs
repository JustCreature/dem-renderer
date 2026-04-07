use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

pub fn print_system_info() {
    let mut sys = System::new_with_specifics(
        RefreshKind::nothing()
            .with_cpu(CpuRefreshKind::everything())
            .with_memory(MemoryRefreshKind::everything()),
    );
    // Two refreshes needed for accurate CPU frequency/usage
    sys.refresh_all();
    std::thread::sleep(std::time::Duration::from_millis(200));
    sys.refresh_all();

    println!("=== System Info ===");

    // Hostname
    if let Some(h) = System::host_name() {
        println!("Hostname       : {h}");
    }

    // OS
    if let Some(os) = System::long_os_version() {
        println!("OS             : {os}");
    }

    // Architecture (compile-time constant, correct for the running binary)
    println!("Arch           : {}", std::env::consts::ARCH);

    // CPU — all logical CPUs share the same brand; frequency from first CPU
    let cpus = sys.cpus();
    if let Some(first) = cpus.first() {
        println!("CPU            : {}", first.brand());
        println!("CPU freq       : {} MHz", first.frequency());
    }
    println!(
        "Cores          : {} physical / {} logical",
        sys.physical_core_count().unwrap_or(0),
        cpus.len(),
    );

    // ── Cache sizes ──────────────────────────────────────────────────────────
    if let Some(caches) = cache_sizes() {
        println!("Cache          : {caches}");
    }

    // ── NUMA nodes ───────────────────────────────────────────────────────────
    if let Some(numa) = numa_nodes() {
        println!("NUMA nodes     : {numa}");
    }

    // ── Power / scheduler mode ───────────────────────────────────────────────
    if let Some(power) = power_mode() {
        println!("Power mode     : {power}");
    }

    // ── RAM ──────────────────────────────────────────────────────────────────
    let total_gb = sys.total_memory() / 1_073_741_824;
    println!("RAM            : {total_gb} GB");

    // ── GPU ──────────────────────────────────────────────────────────────────
    #[cfg(target_os = "macos")]
    if let Some(gpu) = macos_gpu() {
        println!("GPU            : {gpu}");
    }
    #[cfg(target_os = "linux")]
    if let Some(gpu) = linux_gpu() {
        println!("GPU            : {gpu}");
    }
    #[cfg(target_os = "windows")]
    if let Some(gpu) = windows_gpu() {
        println!("GPU            : {gpu}");
    }

    // ── Build info ───────────────────────────────────────────────────────────
    println!(
        "Build profile  : {}",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    if let Some(rustc) = rustc_version() {
        println!("rustc          : {rustc}");
    }
    if let Ok(flags) = std::env::var("RUSTFLAGS") {
        if !flags.is_empty() {
            println!("RUSTFLAGS      : {flags}");
        }
    }

    println!("===================");
    println!();
}

// ── cache sizes ─────────────────────────────────────────────────────────────

fn cache_sizes() -> Option<String> {
    #[cfg(target_os = "macos")]
    return macos_cache_sizes();
    #[cfg(target_os = "linux")]
    return linux_cache_sizes();
    #[cfg(target_os = "windows")]
    return windows_cache_sizes();
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    None
}

#[cfg(target_os = "macos")]
fn macos_cache_sizes() -> Option<String> {
    fn sysctl_u64(key: &str) -> Option<u64> {
        let out = std::process::Command::new("sysctl")
            .arg("-n")
            .arg(key)
            .output()
            .ok()?;
        String::from_utf8(out.stdout).ok()?.trim().parse().ok()
    }
    let l1 = sysctl_u64("hw.l1dcachesize").map(|v| format!("L1D {}", fmt_bytes(v)));
    let l2 = sysctl_u64("hw.l2cachesize").map(|v| format!("L2 {}", fmt_bytes(v)));
    let l3 = sysctl_u64("hw.l3cachesize").map(|v| format!("L3 {}", fmt_bytes(v)));
    let parts: Vec<_> = [l1, l2, l3].into_iter().flatten().collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("  "))
    }
}

#[cfg(target_os = "linux")]
fn linux_cache_sizes() -> Option<String> {
    // Walk /sys/devices/system/cpu/cpu0/cache/index*/
    let mut entries: Vec<(u32, String, u64)> = Vec::new(); // (level, type, size_bytes)
    for i in 0..8 {
        let base = format!("/sys/devices/system/cpu/cpu0/cache/index{i}");
        let level: u32 = std::fs::read_to_string(format!("{base}/level"))
            .ok()?
            .trim()
            .parse()
            .ok()?;
        let kind = std::fs::read_to_string(format!("{base}/type"))
            .unwrap_or_default()
            .trim()
            .to_string();
        let size_str = std::fs::read_to_string(format!("{base}/size"))
            .unwrap_or_default()
            .trim()
            .to_uppercase();
        let size_kb: u64 = if size_str.ends_with('K') {
            size_str.trim_end_matches('K').parse().unwrap_or(0)
        } else {
            size_str.parse::<u64>().unwrap_or(0) / 1024
        };
        if size_kb > 0 && kind != "Instruction" {
            entries.push((level, kind, size_kb * 1024));
        }
    }
    entries.sort_by_key(|(l, _, _)| *l);
    let parts: Vec<_> = entries
        .iter()
        .map(|(l, t, s)| {
            let label = if *t == "Unified" {
                format!("L{l}")
            } else {
                format!("L{l}D")
            };
            format!("{label} {}", fmt_bytes(*s))
        })
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("  "))
    }
}

#[cfg(target_os = "windows")]
fn windows_cache_sizes() -> Option<String> {
    // wmic gives L2/L3 in KB
    let out = std::process::Command::new("wmic")
        .args(["cpu", "get", "L2CacheSize,L3CacheSize", "/format:list"])
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let mut parts = Vec::new();
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("L2CacheSize=") {
            let kb: u64 = v.trim().parse().unwrap_or(0);
            if kb > 0 {
                parts.push(format!("L2 {}", fmt_bytes(kb * 1024)));
            }
        }
        if let Some(v) = line.strip_prefix("L3CacheSize=") {
            let kb: u64 = v.trim().parse().unwrap_or(0);
            if kb > 0 {
                parts.push(format!("L3 {}", fmt_bytes(kb * 1024)));
            }
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("  "))
    }
}

// ── NUMA nodes ───────────────────────────────────────────────────────────────

fn numa_nodes() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        // Count /sys/devices/system/node/nodeN directories
        let count = std::fs::read_dir("/sys/devices/system/node")
            .ok()?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("node"))
            .count();
        return if count > 0 {
            Some(count.to_string())
        } else {
            None
        };
    }
    #[cfg(target_os = "macos")]
    {
        // macOS has no NUMA; report packages (physical CPUs) — always 1 for any Mac
        let out = std::process::Command::new("sysctl")
            .arg("-n")
            .arg("hw.packages")
            .output()
            .ok()?;
        let n = String::from_utf8(out.stdout).ok()?.trim().to_string();
        return Some(format!("{n} (macOS unified memory, no NUMA)"));
    }
    #[cfg(target_os = "windows")]
    {
        let out = std::process::Command::new("wmic")
            .args([
                "computersystem",
                "get",
                "NumberOfProcessors",
                "/format:list",
            ])
            .output()
            .ok()?;
        let text = String::from_utf8(out.stdout).ok()?;
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("NumberOfProcessors=") {
                return Some(v.trim().to_string());
            }
        }
        return None;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    None
}

// ── power / scheduler mode ───────────────────────────────────────────────────

fn power_mode() -> Option<String> {
    #[cfg(target_os = "macos")]
    return macos_power_mode();
    #[cfg(target_os = "linux")]
    return linux_power_mode();
    #[cfg(target_os = "windows")]
    return windows_power_mode();
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    None
}

#[cfg(target_os = "macos")]
fn macos_power_mode() -> Option<String> {
    let out = std::process::Command::new("pmset")
        .arg("-g")
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    // "lowpowermode   1" means Low Power Mode is on
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("lowpowermode") {
            let val = trimmed.split_whitespace().nth(1).unwrap_or("0");
            return Some(if val == "1" {
                "Low Power Mode ON".to_string()
            } else {
                "Normal".to_string()
            });
        }
    }
    Some("Normal".to_string())
}

#[cfg(target_os = "linux")]
fn linux_power_mode() -> Option<String> {
    // Per-CPU governor — read from cpu0
    let gov = std::fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor")
        .ok()?
        .trim()
        .to_string();
    // Also check energy_performance_preference if available
    let epp = std::fs::read_to_string(
        "/sys/devices/system/cpu/cpu0/cpufreq/energy_performance_preference",
    )
    .ok()
    .map(|s| format!(", EPP: {}", s.trim()));
    Some(format!("{gov}{}", epp.unwrap_or_default()))
}

#[cfg(target_os = "windows")]
fn windows_power_mode() -> Option<String> {
    let out = std::process::Command::new("powercfg")
        .args(["/getactivescheme"])
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    // "Power Scheme GUID: ... (Balanced)" — grab the name in parens
    text.lines().next().and_then(|l| {
        let start = l.find('(')?;
        let end = l.rfind(')')?;
        Some(l[start + 1..end].to_string())
    })
}

// ── rustc version ────────────────────────────────────────────────────────────

fn rustc_version() -> Option<String> {
    let out = std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .ok()?;
    String::from_utf8(out.stdout)
        .ok()
        .map(|s| s.trim().to_string())
}

// ── GPU helpers ──────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn macos_gpu() -> Option<String> {
    let out = std::process::Command::new("system_profiler")
        .arg("SPDisplaysDataType")
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let name = text
        .lines()
        .find(|l| l.trim_start().starts_with("Chipset Model"))?
        .split(':')
        .nth(1)?
        .trim()
        .to_string();
    let vram = text
        .lines()
        .find(|l| l.trim_start().starts_with("VRAM"))
        .and_then(|l| l.split(':').nth(1))
        .map(|v| format!(", {}", v.trim()))
        .unwrap_or_default();
    Some(format!("{name}{vram}"))
}

#[cfg(target_os = "linux")]
fn linux_gpu() -> Option<String> {
    let out = std::process::Command::new("lspci").output().ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let line = text.lines().find(|l| {
        let lower = l.to_lowercase();
        lower.contains("vga") || lower.contains("3d controller") || lower.contains("display")
    })?;
    Some(line.split(':').last()?.trim().to_string())
}

#[cfg(target_os = "windows")]
fn windows_gpu() -> Option<String> {
    let out = std::process::Command::new("wmic")
        .args(["path", "win32_VideoController", "get", "Name"])
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    text.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && *l != "Name")
        .next()
        .map(|s| s.to_string())
}

// ── utilities ────────────────────────────────────────────────────────────────

fn fmt_bytes(b: u64) -> String {
    if b >= 1_048_576 {
        format!("{} MB", b / 1_048_576)
    } else {
        format!("{} KB", b / 1_024)
    }
}
