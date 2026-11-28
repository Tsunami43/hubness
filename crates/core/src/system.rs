//! Server monitoring: host, metrics, Docker, services, packages, “Ask about this”
//! (`packages/shared/src/system.ts`).

use serde::{Deserialize, Serialize};

use crate::js_number;

/// Like [`js_number`] for optional numbers (`None` → `null`).
pub fn js_number_opt<S: serde::Serializer>(v: &Option<f64>, s: S) -> Result<S::Ok, S::Error> {
    match v {
        Some(n) => js_number(n, s),
        None => s.serialize_none(),
    }
}

fn js_numbers<S: serde::Serializer>(v: &[f64; 3], s: S) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeTuple;
    let mut t = s.serialize_tuple(3)?;
    for n in v {
        t.serialize_element(&JsNumber(*n))?;
    }
    t.end()
}

struct JsNumber(f64);

impl Serialize for JsNumber {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        js_number(&self.0, s)
    }
}

/// OS family of the hub server: Linux distributions and macOS are supported; others get basics only.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HostPlatform {
    Linux,
    Darwin,
    Other,
}

/// Service manager behind System → Services.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ServiceManager {
    Systemd,
    Launchd,
}

/// State of a self-update started from the web (`POST /api/system/update`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum HubUpdateStage {
    #[default]
    Idle,
    Running,
    /// The new binary is in place, but this process still runs the old one.
    Installed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct HubUpdateState {
    pub state: HubUpdateStage,
    /// The error of a failed update, or the version an installed one put in place.
    pub message: Option<String>,
}

/// `GET /api/system/version`: this hub's version and the latest release.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HubVersion {
    pub version: String,
    pub latest: Option<String>,
    pub update_available: bool,
    pub release_url: Option<String>,
    pub can_update: bool,
    pub update: HubUpdateState,
}

/// What the System pages can do on this server (asked once by the System menu).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct SystemPlatform {
    pub platform: HostPlatform,
    /// `None` — the Services section is unavailable.
    pub services: Option<ServiceManager>,
}

/// Static host facts for the System → Overview header (re-read every few minutes).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HostInfo {
    pub platform: HostPlatform,
    pub services: Option<ServiceManager>,
    /// os-release ID (arch, ubuntu, …) or `macos`.
    pub os_id: String,
    /// os-release ID_LIKE, used to pick a logo for derivatives.
    pub os_like: Vec<String>,
    pub os_name: String,
    pub arch: String,
    pub kernel: String,
    /// Machine model (DMI on Linux, hw.model on macOS).
    pub host: Option<String>,
    pub cpu: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GpuInfo {
    pub name: String,
    /// Busy percent where the driver exposes it (amdgpu sysfs, nvidia-smi).
    #[serde(serialize_with = "js_number_opt")]
    pub usage_percent: Option<f64>,
    #[serde(serialize_with = "js_number_opt")]
    pub temp_c: Option<f64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TempKind {
    Cpu,
    Gpu,
    Disk,
    Other,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TempSensor {
    pub kind: TempKind,
    pub label: String,
    #[serde(serialize_with = "js_number")]
    pub celsius: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryUsage {
    pub total_mb: i64,
    pub available_mb: i64,
    pub used_percent: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SwapUsage {
    pub total_mb: i64,
    pub used_mb: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DiskUsage {
    pub device: String,
    pub mount: String,
    pub size_gb: i64,
    pub used_gb: i64,
    /// `None` (`null`) where df prints no percentage.
    #[serde(serialize_with = "js_number_opt")]
    pub used_percent: Option<f64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SystemOverview {
    pub info: HostInfo,
    pub hostname: String,
    pub uptime_sec: i64,
    #[serde(serialize_with = "js_numbers")]
    pub load: [f64; 3],
    pub cpus: u32,
    pub cpu_percent: Option<i64>,
    pub gpus: Vec<GpuInfo>,
    /// Hardware sensors (Linux hwmon); empty where unreadable (macOS without root).
    pub temps: Vec<TempSensor>,
    pub memory: MemoryUsage,
    pub swap: SwapUsage,
    pub disks: Vec<DiskUsage>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DockerNetwork {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub scope: String,
    pub subnet: Option<String>,
    /// Names of attached containers (running ones are attached).
    pub containers: Vec<String>,
    /// Compose project that created it.
    pub project: Option<String>,
    /// bridge / host / none — Docker's own, can't be removed.
    pub builtin: bool,
    pub internal: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DockerVolume {
    pub name: String,
    pub driver: String,
    /// Containers using it (running or stopped); 0 — unused.
    pub links: i64,
    pub size: Option<String>,
    pub project: Option<String>,
    /// Created without a name (hash-like name).
    pub anonymous: bool,
    pub mountpoint: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContainerInfo {
    pub id: String,
    pub name: String,
    pub image: String,
    /// running | exited | restarting…
    pub state: String,
    /// «Up 9 hours (healthy)»
    pub status: String,
    pub health: Option<String>,
    pub restart_count: i64,
    pub oom_killed: bool,
    pub exit_code: i64,
    pub ports: String,
    pub mem_limit_mb: Option<i64>,
    /// When the container stopped (ISO), if it is not running.
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UnitScope {
    System,
    User,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UnitInfo {
    pub name: String,
    pub scope: UnitScope,
    pub load: String,
    pub active: String,
    pub sub: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PackageInfo {
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_version: Option<String>,
    /// Extra detail: a local path (cargo), «active, default» (rustup), the module (go).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PackageManagerKind {
    System,
    Language,
    Tool,
}

/// A package manager found on the server (one tab in System → Packages).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PackageManagerInfo {
    pub id: String,
    pub label: String,
    pub kind: PackageManagerKind,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PackageListTitle {
    Explicit,
    Installed,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PackagesResponse {
    pub installed: usize,
    /// The main list: explicitly installed (system managers) or everything the manager installed.
    pub packages: Vec<PackageInfo>,
    pub list_title: PackageListTitle,
    /// `None` — this manager can't check for updates (see `updates_note`).
    pub updates: Option<Vec<PackageInfo>>,
    pub updates_note: String,
    pub manager: String,
    pub label: String,
    /// When the data was read (it is cached on the server).
    pub checked_at: String,
}

/// Target of "Ask about this".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum AskTarget {
    Container { id: String },
    Unit { id: String, scope: UnitScope },
    Package { id: String },
}
