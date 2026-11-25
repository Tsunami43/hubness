//! Host facts and sensors for System → Overview and platform detection for the System
//! pages. Linux distributions and macOS are supported; anything else gets the portable
//! basics only.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

use hubness_core::{GpuInfo, HostInfo, HostPlatform, ServiceManager, SystemPlatform, TempKind, TempSensor};
use regex::Regex;

use super::exec::{on_path, sh, sh_timeout};

pub const PLATFORM: HostPlatform = if cfg!(target_os = "linux") {
    HostPlatform::Linux
} else if cfg!(target_os = "macos") {
    HostPlatform::Darwin
} else {
    HostPlatform::Other
};

/// A file's text, trimmed; empty when unreadable.
pub(crate) fn read(path: impl AsRef<Path>) -> String {
    fs::read(path).map(|b| String::from_utf8_lossy(&b).trim().to_string()).unwrap_or_default()
}

/// Folder entries sorted by name (like Node's `readdirSync`).
pub(crate) fn read_dir_sorted(path: impl AsRef<Path>) -> Option<Vec<String>> {
    let mut names: Vec<String> = fs::read_dir(path).ok()?.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    Some(names)
}

/// One-liner for rare static facts (sysctl, sw_vers): trimmed stdout, empty on failure.
async fn run(cmd: &str, args: &[&str]) -> String {
    let r = sh_timeout(cmd, args, Duration::from_secs(5)).await;
    if r.code == 0 { r.out.trim().to_string() } else { String::new() }
}

/// systemd only when it is PID 1 (containers and OpenRC/runit hosts have systemctl without it).
pub fn system_platform() -> SystemPlatform {
    static CACHE: OnceLock<SystemPlatform> = OnceLock::new();
    *CACHE.get_or_init(|| {
        let mut services = None;
        if PLATFORM == HostPlatform::Linux && Path::new("/run/systemd/system").exists() && on_path("systemctl") {
            services = Some(ServiceManager::Systemd);
        }
        if PLATFORM == HostPlatform::Darwin && on_path("launchctl") {
            services = Some(ServiceManager::Launchd);
        }
        SystemPlatform { platform: PLATFORM, services }
    })
}

pub fn parse_os_release(text: &str) -> HashMap<String, String> {
    static LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([A-Z_]+)=(.*)$").unwrap());
    static QUOTES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"^["']|["']$"#).unwrap());
    text.split('\n').filter_map(|l| LINE.captures(l)).map(|m| (m[1].to_string(), QUOTES.replace_all(&m[2], "").into_owned())).collect()
}

/// `uname`: (sysname, release).
fn uname() -> (String, String) {
    // SAFETY: uname fills the zeroed struct with NUL-terminated strings.
    unsafe {
        let mut u: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut u) != 0 {
            return (String::new(), String::new());
        }
        let s = |f: &[libc::c_char]| std::ffi::CStr::from_ptr(f.as_ptr()).to_string_lossy().into_owned();
        (s(&u.sysname), s(&u.release))
    }
}

pub fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is valid for its length; the result is NUL-terminated on success.
    if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } != 0 {
        return String::new();
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

async fn os_facts() -> (String, Vec<String>, String) {
    match PLATFORM {
        HostPlatform::Darwin => {
            let name = run("sw_vers", &["-productName"]).await;
            let name = if name.is_empty() { "macOS".to_string() } else { name };
            let version = run("sw_vers", &["-productVersion"]).await;
            let os_name = [name, version].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
            ("macos".into(), vec![], os_name)
        }
        HostPlatform::Linux => {
            let text = Some(read("/etc/os-release")).filter(|t| !t.is_empty()).unwrap_or_else(|| read("/usr/lib/os-release"));
            let r = parse_os_release(&text);
            let id = r.get("ID").map(|s| s.to_lowercase()).unwrap_or_else(|| "linux".into());
            let like = r.get("ID_LIKE").map(|s| s.to_lowercase()).unwrap_or_default().split_whitespace().map(String::from).collect();
            let name = [r.get("PRETTY_NAME"), r.get("NAME")]
                .into_iter()
                .flatten()
                .find(|s| !s.is_empty())
                .cloned()
                .unwrap_or_else(|| "Linux".into());
            (id, like, name)
        }
        HostPlatform::Other => (std::env::consts::OS.to_string(), vec![], uname().0),
    }
}

/// Vendor junk the firmware leaves in DMI fields.
static DMI_JUNK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(to be filled.*|default string|system product name|system version|not applicable|none|o\.e\.m\.?|x\.x)$").unwrap()
});

async fn host_model() -> Option<String> {
    if PLATFORM == HostPlatform::Darwin {
        return Some(run("sysctl", &["-n", "hw.model"]).await).filter(|s| !s.is_empty());
    }
    if PLATFORM != HostPlatform::Linux {
        return None;
    }
    let dmi = |f: &str| {
        let v = read(format!("/sys/devices/virtual/dmi/id/{f}"));
        if !v.is_empty() && !DMI_JUNK.is_match(&v) { v } else { String::new() }
    };
    // Raspberry Pi and other boards expose the model via device tree
    let dt = read("/sys/firmware/devicetree/base/model").replace('\0', "");
    let product = dmi("product_name");
    let name = if product.is_empty() { dt } else { product };
    if name.is_empty() {
        return None;
    }
    let version = dmi("product_version");
    Some(if !version.is_empty() && !name.contains(&version) { format!("{name} ({version})") } else { name })
}

/// Model name of the first CPU (what Node's `os.cpus()[0].model` reads).
fn first_cpu_model() -> String {
    let info = read("/proc/cpuinfo");
    for key in ["model name", "Processor", "cpu model"] {
        if let Some(v) = info.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.trim() == key).map(|(_, v)| v.trim().to_string())) {
            return v;
        }
    }
    String::new()
}

async fn cpu_model() -> Option<String> {
    let darwin = if PLATFORM == HostPlatform::Darwin { run("sysctl", &["-n", "machdep.cpu.brand_string"]).await } else { String::new() };
    let raw = if darwin.is_empty() { first_cpu_model() } else { darwin };
    if raw.is_empty() {
        return None;
    }
    Some(clean_cpu_model(&raw))
}

pub fn clean_cpu_model(raw: &str) -> String {
    static RULES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
        [r"(?i)\((R|TM)\)", r"(?i)\s+(CPU|Processor)\b", r"(?i)\s+\d+-Core\b", r"(?i)\s+w/\s.*$", r"\s+@\s.*$"]
            .iter()
            .map(|r| Regex::new(r).unwrap())
            .collect()
    });
    static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
    let mut s = raw.to_string();
    for r in RULES.iter() {
        s = r.replace_all(&s, "").into_owned();
    }
    SPACES.replace_all(&s, " ").trim().to_string()
}

/// `Advanced Micro Devices, Inc. [AMD/ATI]` + `Strix [Radeon 880M / 890M]` → `AMD Radeon 880M / 890M`.
pub fn gpu_name(vendor: &str, device: &str) -> String {
    static BRACKET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[([^\]]+)\]").unwrap());
    static SUFFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i),? (Inc|Corporation|Corp)\.?$").unwrap());
    let bracket = |s: &str| BRACKET.captures(s).map(|m| m[1].to_string());
    let v = bracket(vendor).unwrap_or_else(|| vendor.to_string());
    let v = v.strip_suffix("/ATI").unwrap_or(&v);
    let v = SUFFIX.replace(v, "").trim().to_string();
    let d = bracket(device).unwrap_or_else(|| device.to_string());
    if d.to_lowercase().starts_with(&v.to_lowercase()) { d } else { format!("{v} {d}") }
}

#[derive(Debug, Clone)]
struct GpuDevice {
    /// PCI address `0000:c5:00.0` (lowercase) to match sensors; `None` on macOS.
    addr: Option<String>,
    name: String,
}

async fn gpu_devices() -> Vec<GpuDevice> {
    if PLATFORM == HostPlatform::Darwin {
        let r = sh_timeout("system_profiler", &["SPDisplaysDataType", "-json"], Duration::from_secs(15)).await;
        let v: serde_json::Value = serde_json::from_str(&r.out).unwrap_or_default();
        return v["SPDisplaysDataType"]
            .as_array()
            .map(|list| {
                list.iter()
                    .map(|g| GpuDevice { addr: None, name: g["sppci_model"].as_str().or(g["_name"].as_str()).unwrap_or("").to_string() })
                    .filter(|g| !g.name.is_empty())
                    .collect()
            })
            .unwrap_or_default();
    }
    if PLATFORM != HostPlatform::Linux || !on_path("lspci") {
        return vec![];
    }
    parse_lspci(&sh("lspci", &["-mm", "-D"]).await.out)
}

fn parse_lspci(out: &str) -> Vec<GpuDevice> {
    static QUOTED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""([^"]*)""#).unwrap());
    static DISPLAY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)VGA|3D|Display").unwrap());
    out.split('\n')
        .filter_map(|l| {
            let f: Vec<&str> = QUOTED.captures_iter(l).map(|m| m.get(1).map_or("", |g| g.as_str())).collect();
            if !DISPLAY.is_match(f.first().copied().unwrap_or("")) {
                return None;
            }
            let addr = l.split(' ').next().unwrap_or("").to_lowercase();
            let at = |i: usize| f.get(i).copied().unwrap_or("");
            Some(GpuDevice { addr: Some(addr), name: gpu_name(at(1), at(2)) })
        })
        .collect()
}

#[derive(Clone)]
struct Static {
    info: HostInfo,
    gpus: Vec<GpuDevice>,
}

const TTL: Duration = Duration::from_secs(10 * 60);
static CACHE: Mutex<Option<(Instant, Static)>> = Mutex::new(None);

fn arch() -> String {
    match std::env::consts::ARCH {
        "aarch64" if PLATFORM == HostPlatform::Darwin => "arm64".into(),
        "x86" => "ia32".into(),
        "powerpc64" => "ppc64".into(),
        "loongarch64" => "loong64".into(),
        a => a.into(),
    }
}

async fn read_static() -> Static {
    let p = system_platform();
    let (os_id, os_like, os_name) = os_facts().await;
    let (sysname, release) = uname();
    let kernel = format!("{} {release}", if PLATFORM == HostPlatform::Darwin { "Darwin" } else { sysname.as_str() });
    Static {
        info: HostInfo {
            platform: p.platform,
            services: p.services,
            os_id,
            os_like,
            os_name,
            arch: arch(),
            kernel,
            host: host_model().await,
            cpu: cpu_model().await,
        },
        gpus: gpu_devices().await,
    }
}

/// Static facts are cached for 10 minutes (lspci / system_profiler are the slow part).
async fn static_facts() -> Static {
    if let Some((at, s)) = CACHE.lock().ok().and_then(|c| c.clone())
        && at.elapsed() <= TTL
    {
        return s;
    }
    let s = read_static().await;
    if let Ok(mut c) = CACHE.lock() {
        *c = Some((Instant::now(), s.clone()));
    }
    s
}

pub async fn host_info() -> HostInfo {
    static_facts().await.info
}

// ---------- live sensors (Linux hwmon, nvidia-smi) ----------

const CPU_SENSORS: &[&str] = &["k10temp", "coretemp", "zenpower", "cpu_thermal", "soc_thermal"];
const GPU_SENSORS: &[&str] = &["amdgpu", "radeon", "nouveau", "i915", "xe"];
const DISK_SENSORS: &[&str] = &["nvme", "drivetemp"];
/// Preferred input per chip: CPU package / die, GPU edge, NVMe composite.
static PREFERRED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(tctl|tdie|package id 0|edge|composite)$").unwrap());

struct HwmonChip {
    name: String,
    /// PCI address of the device behind the chip (GPUs).
    addr: Option<String>,
    celsius: Option<f64>,
    busy: Option<f64>,
}

/// `Number(text)` for sysfs values: empty → 0, junk → NaN.
fn js_num(text: &str) -> f64 {
    let t = text.trim();
    if t.is_empty() { 0.0 } else { t.parse().unwrap_or(f64::NAN) }
}

fn hwmon_chips() -> Vec<HwmonChip> {
    static INPUT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^temp\d+_input$").unwrap());
    let base = Path::new("/sys/class/hwmon");
    let Some(dirs) = read_dir_sorted(base) else { return vec![] };
    dirs.into_iter()
        .map(|d| {
            let dir = base.join(d);
            let inputs: Vec<(String, f64)> = read_dir_sorted(&dir)
                .unwrap_or_default()
                .into_iter()
                .filter(|f| INPUT.is_match(f))
                .map(|f| (read(dir.join(f.replace("_input", "_label"))), js_num(&read(dir.join(&f)))))
                .filter(|(_, v)| v.is_finite() && *v > 0.0)
                .collect();
            let pick = inputs.iter().find(|(l, _)| PREFERRED.is_match(l)).or(inputs.first());
            let addr = fs::canonicalize(dir.join("device")).ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
            let busy = read(dir.join("device").join("gpu_busy_percent"));
            HwmonChip {
                name: read(dir.join("name")),
                addr,
                celsius: pick.map(|(_, v)| (v / 100.0).round() / 10.0),
                busy: if busy.is_empty() { None } else { Some(js_num(&busy)).filter(|n| n.is_finite()) },
            }
        })
        .collect()
}

struct NvidiaRow {
    addr: String,
    usage: Option<f64>,
    temp: Option<f64>,
}

async fn nvidia_rows() -> Vec<NvidiaRow> {
    if !on_path("nvidia-smi") {
        return vec![];
    }
    let r = sh_timeout(
        "nvidia-smi",
        &["--query-gpu=pci.bus_id,utilization.gpu,temperature.gpu", "--format=csv,noheader,nounits"],
        Duration::from_secs(5),
    )
    .await;
    parse_nvidia(&r.out)
}

fn parse_nvidia(out: &str) -> Vec<NvidiaRow> {
    static NUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+(\.\d+)?$").unwrap());
    // nvidia-smi pads the domain to 8 digits: 00000000:01:00.0 → 0000:01:00.0
    static DOMAIN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^0{4}([0-9a-f]{4}:)").unwrap());
    let num = |v: Option<&&str>| v.map(|s| s.trim()).filter(|s| NUM.is_match(s)).and_then(|s| s.parse().ok());
    out.split('\n')
        .filter(|l| !l.is_empty())
        .map(|l| {
            let c: Vec<&str> = l.split(',').map(str::trim).collect();
            let bus = c.first().copied().unwrap_or("").to_lowercase();
            NvidiaRow { addr: DOMAIN.replace(&bus, "$1").into_owned(), usage: num(c.get(1)), temp: num(c.get(2)) }
        })
        .collect()
}

pub async fn sensors() -> (Vec<GpuInfo>, Vec<TempSensor>) {
    let gpus = static_facts().await.gpus;
    let chips = if PLATFORM == HostPlatform::Linux { hwmon_chips() } else { vec![] };
    let nvidia = nvidia_rows().await;
    let mut temps = vec![];
    let mut add = |kind: TempKind, label: String, c: Option<f64>| {
        if let Some(c) = c.filter(|c| *c > 0.0 && *c < 150.0) {
            temps.push(TempSensor { kind, label, celsius: c });
        }
    };
    let cpu = chips
        .iter()
        .find(|c| CPU_SENSORS.contains(&c.name.as_str()) && c.celsius.is_some())
        .or_else(|| chips.iter().find(|c| c.name == "acpitz"));
    add(TempKind::Cpu, "CPU".into(), cpu.and_then(|c| c.celsius));
    let gpu_info: Vec<GpuInfo> = gpus
        .iter()
        .map(|g| {
            let chip = chips.iter().find(|c| GPU_SENSORS.contains(&c.name.as_str()) && c.addr == g.addr);
            let nv = nvidia.iter().find(|n| Some(&n.addr) == g.addr.as_ref());
            GpuInfo {
                name: g.name.clone(),
                usage_percent: nv.and_then(|n| n.usage).or(chip.and_then(|c| c.busy)),
                temp_c: nv.and_then(|n| n.temp).or(chip.and_then(|c| c.celsius)),
            }
        })
        .collect();
    for (i, g) in gpu_info.iter().enumerate() {
        add(TempKind::Gpu, if gpu_info.len() > 1 { format!("GPU {}", i + 1) } else { "GPU".into() }, g.temp_c);
    }
    let disks: Vec<&HwmonChip> = chips.iter().filter(|c| DISK_SENSORS.contains(&c.name.as_str())).collect();
    for (i, c) in disks.iter().enumerate() {
        let name = if c.name == "nvme" { "NVMe" } else { "Disk" };
        add(TempKind::Disk, if disks.len() > 1 { format!("{name} {}", i + 1) } else { name.into() }, c.celsius);
    }
    (gpu_info, temps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_release() {
        let r = parse_os_release("NAME=\"Arch Linux\"\nID=arch\nID_LIKE='debian ubuntu'\n# comment\n");
        assert_eq!(r["NAME"], "Arch Linux");
        assert_eq!(r["ID"], "arch");
        assert_eq!(r["ID_LIKE"], "debian ubuntu");
    }

    #[test]
    fn names() {
        assert_eq!(gpu_name("Advanced Micro Devices, Inc. [AMD/ATI]", "Strix [Radeon 880M / 890M]"), "AMD Radeon 880M / 890M");
        assert_eq!(gpu_name("NVIDIA Corporation", "GA102 [GeForce RTX 3080]"), "NVIDIA GeForce RTX 3080");
        assert_eq!(gpu_name("Intel Corporation", "Intel Iris Xe"), "Intel Iris Xe");
        assert_eq!(clean_cpu_model("Intel(R) Core(TM) i7-8550U CPU @ 1.80GHz"), "Intel Core i7-8550U");
        assert_eq!(clean_cpu_model("AMD Ryzen AI 9 HX 370 w/ Radeon 890M"), "AMD Ryzen AI 9 HX 370");
        assert_eq!(clean_cpu_model("AMD Ryzen 7 5800X 8-Core Processor"), "AMD Ryzen 7 5800X");
    }

    #[test]
    fn lspci_and_nvidia() {
        let gpus = parse_lspci(
            "0000:00:02.0 \"VGA compatible controller\" \"Intel Corporation\" \"UHD Graphics 620\" -r07 \"Lenovo\" \"x\"\n0000:00:1f.3 \"Audio device\" \"Intel\" \"x\"\n",
        );
        assert_eq!(gpus.len(), 1);
        assert_eq!(gpus[0].addr.as_deref(), Some("0000:00:02.0"));
        assert_eq!(gpus[0].name, "Intel UHD Graphics 620");
        let nv = parse_nvidia("00000000:01:00.0, 37, 52\n");
        assert_eq!(nv[0].addr, "0000:01:00.0");
        assert_eq!(nv[0].usage, Some(37.0));
        assert_eq!(nv[0].temp, Some(52.0));
    }
}
