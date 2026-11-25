//! Package managers of the server (System → Packages): one tab per manager found on PATH.
//!
//! Detection is cheap (looks for the binaries). A manager's data is read only when its tab is
//! opened and cached for `CACHE` — some checks go to the network (AUR, npm registry, rustup
//! dist server). «Refresh» bypasses the cache. Nothing here installs or updates anything.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use hubness_core::{PackageInfo, PackageListTitle, PackageManagerInfo, PackageManagerKind, PackagesResponse};
use regex::Regex;
use serde_json::Value;

use super::exec::{on_path, sh, sh_timeout};
use super::host::read_dir_sorted;
use crate::error::{ApiResult, HttpError};

const CACHE: Duration = Duration::from_secs(15 * 60);
const NET_TIMEOUT: Duration = Duration::from_secs(60);

/// What a manager reads (the response without manager, label and checkedAt).
struct Read {
    installed: usize,
    packages: Vec<PackageInfo>,
    list_title: PackageListTitle,
    updates: Option<Vec<PackageInfo>>,
    updates_note: String,
}

struct Manager {
    id: &'static str,
    label: &'static str,
    kind: PackageManagerKind,
    /// Binary that must be on PATH (the first found wins for alternatives).
    bins: &'static [&'static str],
}

const MANAGERS: &[Manager] = &[
    // ---------- system ----------
    Manager { id: "pacman", label: "pacman", kind: PackageManagerKind::System, bins: &["pacman"] },
    Manager { id: "aur", label: "AUR", kind: PackageManagerKind::System, bins: &["paru", "yay"] },
    Manager { id: "apt", label: "apt", kind: PackageManagerKind::System, bins: &["apt"] },
    Manager { id: "dnf", label: "dnf", kind: PackageManagerKind::System, bins: &["dnf"] },
    Manager { id: "flatpak", label: "Flatpak", kind: PackageManagerKind::System, bins: &["flatpak"] },
    Manager { id: "snap", label: "Snap", kind: PackageManagerKind::System, bins: &["snap"] },
    Manager { id: "brew", label: "Homebrew", kind: PackageManagerKind::System, bins: &["brew"] },
    // ---------- languages ----------
    Manager { id: "cargo", label: "cargo", kind: PackageManagerKind::Language, bins: &["cargo"] },
    Manager { id: "rustup", label: "rustup", kind: PackageManagerKind::Language, bins: &["rustup"] },
    Manager { id: "npm", label: "npm (global)", kind: PackageManagerKind::Language, bins: &["npm"] },
    Manager { id: "uv", label: "uv tools", kind: PackageManagerKind::Tool, bins: &["uv"] },
    Manager { id: "pipx", label: "pipx", kind: PackageManagerKind::Tool, bins: &["pipx"] },
    Manager { id: "pip", label: "pip (user)", kind: PackageManagerKind::Language, bins: &["pip"] },
    Manager { id: "go", label: "go install", kind: PackageManagerKind::Language, bins: &["go"] },
];

fn pkg(name: &str, version: &str) -> PackageInfo {
    PackageInfo { name: name.into(), version: version.into(), new_version: None, note: None }
}

fn upd(name: &str, version: &str, new_version: Option<&str>) -> PackageInfo {
    PackageInfo { new_version: new_version.map(String::from), ..pkg(name, version) }
}

/// Non-empty trimmed lines.
fn lines(s: &str) -> Vec<&str> {
    s.split('\n').map(str::trim).filter(|l| !l.is_empty()).collect()
}

fn re(r: &str) -> Regex {
    Regex::new(r).expect("valid regex")
}

/// "name version" or "name version -> new" lines (pacman, yay -Qua).
fn name_version(out: &str) -> Vec<PackageInfo> {
    static R: LazyLock<Regex> = LazyLock::new(|| re(r"^(\S+)\s+(\S+)(?:\s+->\s+(\S+))?"));
    lines(out).into_iter().filter_map(|l| R.captures(l)).map(|m| upd(&m[1], &m[2], m.get(3).map(|g| g.as_str()))).collect()
}

fn json(s: &str) -> Option<Value> {
    serde_json::from_str(s).ok()
}

fn str_of(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

fn versions(list: &[PackageInfo]) -> HashMap<String, String> {
    list.iter().map(|p| (p.name.clone(), p.version.clone())).collect()
}

async fn read_manager(id: &str, bin: &str) -> Read {
    match id {
        "pacman" => {
            let (all, explicit, upd) = tokio::join!(sh("pacman", &["-Q"]), sh("pacman", &["-Qen"]), sh("pacman", &["-Qu"]));
            Read {
                installed: name_version(&all.out).len(),
                packages: name_version(&explicit.out),
                list_title: PackageListTitle::Explicit,
                updates: Some(name_version(&upd.out)),
                updates_note: "From the last repository sync (pacman -Qu). `sudo pacman -Sy` refreshes it.".into(),
            }
        }
        "aur" => {
            let (foreign, up) = tokio::join!(sh("pacman", &["-Qm"]), sh_timeout(bin, &["-Qua"], NET_TIMEOUT));
            let list = name_version(&foreign.out);
            Read {
                installed: list.len(),
                packages: list,
                list_title: PackageListTitle::Installed,
                updates: (up.code <= 1).then(|| name_version(&up.out)),
                updates_note: format!("Checked against the AUR by {bin} -Qua."),
            }
        }
        "apt" => {
            let (all, manual, up) = tokio::join!(
                sh("dpkg-query", &["-W", "-f=${Package} ${Version}\n"]),
                sh("apt-mark", &["showmanual"]),
                sh("apt", &["list", "--upgradable"])
            );
            let versions = versions(&name_version(&all.out));
            static R: LazyLock<Regex> = LazyLock::new(|| re(r"^([^/\s]+)/\S+\s+(\S+)\s+\S+\s+\[upgradable from: ([^\]]+)\]"));
            let updates = lines(&up.out).into_iter().filter_map(|l| R.captures(l)).map(|m| upd(&m[1], &m[3], Some(&m[2]))).collect();
            Read {
                installed: versions.len(),
                packages: lines(&manual.out).into_iter().map(|n| pkg(n, versions.get(n).map_or("", String::as_str))).collect(),
                list_title: PackageListTitle::Explicit,
                updates: Some(updates),
                updates_note: "From the last `apt update` (apt list --upgradable).".into(),
            }
        }
        "dnf" => {
            let (all, user, up) = tokio::join!(
                sh("rpm", &["-qa", "--qf", "%{NAME} %{VERSION}-%{RELEASE}\n"]),
                sh("dnf", &["repoquery", "--userinstalled", "--qf", "%{name} %{evr}\n"]),
                sh_timeout("dnf", &["check-update", "-q"], NET_TIMEOUT)
            );
            let versions = versions(&name_version(&all.out));
            static R: LazyLock<Regex> = LazyLock::new(|| re(r"^(\S+?)\.\S+\s+(\S+)\s+\S+$"));
            let updates = lines(&up.out)
                .into_iter()
                .filter_map(|l| R.captures(l))
                .map(|m| upd(&m[1], versions.get(&m[1]).map_or("", String::as_str), Some(&m[2])))
                .collect();
            Read {
                installed: versions.len(),
                packages: name_version(&user.out),
                list_title: PackageListTitle::Explicit,
                // check-update exits 100 when there are updates
                updates: (up.code == 0 || up.code == 100).then_some(updates),
                updates_note: "dnf check-update (uses the cached metadata when fresh).".into(),
            }
        }
        "flatpak" => {
            let (list, up) = tokio::join!(
                sh("flatpak", &["list", "--app", "--columns=application,version"]),
                sh_timeout("flatpak", &["remote-ls", "--updates", "--app", "--columns=application,version"], NET_TIMEOUT)
            );
            let parse = |s: &str| -> Vec<PackageInfo> {
                lines(s)
                    .into_iter()
                    .map(|l| {
                        let mut c = l.split('\t');
                        pkg(c.next().unwrap_or(""), c.next().unwrap_or(""))
                    })
                    .collect()
            };
            with_current(parse(&list.out), (up.code == 0).then(|| parse(&up.out)), "flatpak remote-ls --updates.")
        }
        "snap" => {
            let (list, up) = tokio::join!(sh("snap", &["list"]), sh_timeout("snap", &["refresh", "--list"], NET_TIMEOUT));
            let parse = |s: &str| -> Vec<PackageInfo> {
                lines(s)
                    .into_iter()
                    .skip(1)
                    .map(|l| {
                        let mut c = l.split_whitespace();
                        pkg(c.next().unwrap_or(""), c.next().unwrap_or(""))
                    })
                    .collect()
            };
            with_current(parse(&list.out), (up.code == 0).then(|| parse(&up.out)), "snap refresh --list.")
        }
        "brew" => {
            let (list, up) = tokio::join!(sh("brew", &["list", "--versions"]), sh_timeout("brew", &["outdated", "--verbose"], NET_TIMEOUT));
            let pkgs: Vec<PackageInfo> = lines(&list.out)
                .into_iter()
                .map(|l| {
                    let mut c = l.split_whitespace();
                    let name = c.next().unwrap_or("");
                    pkg(name, &c.collect::<Vec<_>>().join(" "))
                })
                .collect();
            static R: LazyLock<Regex> = LazyLock::new(|| re(r"^(\S+) \((.+)\) [<!=]+ (\S+)"));
            let updates = lines(&up.out).into_iter().filter_map(|l| R.captures(l)).map(|m| upd(&m[1], &m[2], Some(&m[3]))).collect();
            Read {
                installed: pkgs.len(),
                packages: pkgs,
                list_title: PackageListTitle::Installed,
                updates: (up.code == 0).then_some(updates),
                updates_note: "brew outdated.".into(),
            }
        }
        "cargo" => {
            let r = sh("cargo", &["install", "--list"]).await;
            // "name v1.2.3:" or "name v1.2.3 (/local/path):", followed by indented binary names
            static R: LazyLock<Regex> = LazyLock::new(|| re(r"^(\S+) v(\S+?)(?: \(([^)]+)\))?:$"));
            let pkgs: Vec<PackageInfo> = r
                .out
                .split('\n')
                .filter_map(|l| R.captures(l))
                .map(|m| PackageInfo { note: m.get(3).map(|g| g.as_str().to_string()), ..pkg(&m[1], &m[2]) })
                .collect();
            let has_updater = on_path("cargo-install-update");
            let mut updates = None;
            if has_updater {
                let u = sh_timeout("cargo", &["install-update", "--list"], NET_TIMEOUT).await;
                static U: LazyLock<Regex> = LazyLock::new(|| re(r"^(\S+)\s+v(\S+)\s+v(\S+)\s+Yes$"));
                updates = Some(lines(&u.out).into_iter().filter_map(|l| U.captures(l)).map(|m| upd(&m[1], &m[2], Some(&m[3]))).collect());
            }
            Read {
                installed: pkgs.len(),
                packages: pkgs,
                list_title: PackageListTitle::Installed,
                updates,
                updates_note: if has_updater {
                    "cargo install-update --list.".into()
                } else {
                    "Install cargo-update (`cargo install cargo-update`) to see updates of cargo binaries.".into()
                },
            }
        }
        "rustup" => {
            let (list, check) = tokio::join!(sh("rustup", &["toolchain", "list"]), sh_timeout("rustup", &["check"], NET_TIMEOUT));
            static T: LazyLock<Regex> = LazyLock::new(|| re(r"^(\S+)(?: \((.+)\))?"));
            let toolchains: Vec<PackageInfo> = lines(&list.out)
                .into_iter()
                .filter_map(|l| T.captures(l))
                .map(|m| PackageInfo { note: m.get(2).map(|g| g.as_str().to_string()), ..pkg(&m[1], "") })
                .collect();
            static U: LazyLock<Regex> = LazyLock::new(|| re(r"^(\S+) - update available\s*: (\S+).*? -> (\S+)"));
            let updates = lines(&check.out).into_iter().filter_map(|l| U.captures(l)).map(|m| upd(&m[1], &m[2], Some(&m[3]))).collect();
            Read {
                installed: toolchains.len(),
                packages: toolchains,
                list_title: PackageListTitle::Installed,
                updates: (check.code <= 100).then_some(updates),
                updates_note: "rustup check.".into(),
            }
        }
        "npm" => {
            let (ls, outdated) = tokio::join!(
                sh("npm", &["ls", "-g", "--depth=0", "--json"]),
                sh_timeout("npm", &["outdated", "-g", "--json"], NET_TIMEOUT)
            );
            let deps = json(&ls.out).and_then(|v| v["dependencies"].as_object().cloned()).unwrap_or_default();
            let pkgs: Vec<PackageInfo> = deps.iter().map(|(name, d)| pkg(name, d["version"].as_str().unwrap_or(""))).collect();
            let old = json(if outdated.out.is_empty() { "{}" } else { &outdated.out });
            let updates = old.map(|o| {
                o.as_object()
                    .map(|o| o.iter().map(|(name, o)| upd(name, o["current"].as_str().unwrap_or(""), o["latest"].as_str())).collect())
                    .unwrap_or_default()
            });
            Read {
                installed: pkgs.len(),
                packages: pkgs,
                list_title: PackageListTitle::Installed,
                updates,
                updates_note: "npm outdated -g.".into(),
            }
        }
        "uv" => {
            let (list, outdated) =
                tokio::join!(sh("uv", &["tool", "list"]), sh_timeout("uv", &["tool", "list", "--outdated"], NET_TIMEOUT));
            static R: LazyLock<Regex> = LazyLock::new(|| re(r"^(\S+) v(\S+)(?: \[latest: v?(\S+)\])?"));
            let parse = |s: &str| -> Vec<PackageInfo> {
                lines(s)
                    .into_iter()
                    .filter(|l| !l.starts_with('-'))
                    .filter_map(|l| R.captures(l))
                    .map(|m| upd(&m[1], &m[2], m.get(3).map(|g| g.as_str())))
                    .collect()
            };
            let tools = parse(&list.out);
            Read {
                installed: tools.len(),
                packages: tools,
                list_title: PackageListTitle::Installed,
                updates: (outdated.code == 0).then(|| parse(&outdated.out).into_iter().filter(|p| p.new_version.is_some()).collect()),
                updates_note: "uv tool list --outdated.".into(),
            }
        }
        "pipx" => {
            let r = sh("pipx", &["list", "--json"]).await;
            let venvs = json(&r.out).and_then(|v| v["venvs"].as_object().cloned()).unwrap_or_default();
            let pkgs: Vec<PackageInfo> =
                venvs.iter().map(|(name, v)| pkg(name, v["metadata"]["main_package"]["package_version"].as_str().unwrap_or(""))).collect();
            Read {
                installed: pkgs.len(),
                packages: pkgs,
                list_title: PackageListTitle::Installed,
                updates: None,
                updates_note: "pipx has no update check; `pipx upgrade-all` upgrades everything.".into(),
            }
        }
        "pip" => {
            let (list, outdated) = tokio::join!(
                sh("pip", &["list", "--user", "--format=json"]),
                sh_timeout("pip", &["list", "--user", "--outdated", "--format=json"], NET_TIMEOUT)
            );
            let pkgs: Vec<PackageInfo> = json(&list.out)
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default()
                .iter()
                .map(|p| pkg(&str_of(&p["name"]), &str_of(&p["version"])))
                .collect();
            let updates = json(&outdated.out)
                .and_then(|v| v.as_array().cloned())
                .map(|list| list.iter().map(|o| upd(&str_of(&o["name"]), &str_of(&o["version"]), o["latest_version"].as_str())).collect());
            Read {
                installed: pkgs.len(),
                packages: pkgs,
                list_title: PackageListTitle::Installed,
                updates,
                updates_note: "pip list --user --outdated. System Python packages belong to the system package manager.".into(),
            }
        }
        "go" => {
            let gopath = sh("go", &["env", "GOPATH"]).await.out.trim().to_string();
            let gobin = sh("go", &["env", "GOBIN"]).await.out.trim().to_string();
            let gobin = if !gobin.is_empty() {
                gobin
            } else if !gopath.is_empty() {
                Path::new(&gopath).join("bin").to_string_lossy().into_owned()
            } else {
                String::new()
            };
            let files: Vec<String> = if !gobin.is_empty() && Path::new(&gobin).exists() {
                read_dir_sorted(&gobin).unwrap_or_default().into_iter().take(200).collect()
            } else {
                vec![]
            };
            static MOD: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^\s*mod\s+(\S+)\s+(\S+)"));
            let pkgs = futures_util::future::join_all(files.iter().map(|f| {
                let path = Path::new(&gobin).join(f).to_string_lossy().into_owned();
                async move {
                    let r = sh("go", &["version", "-m", &path]).await;
                    let m = MOD.captures(&r.out);
                    PackageInfo {
                        note: m.as_ref().map(|m| m[1].to_string()),
                        ..pkg(f, m.as_ref().map_or("", |m| m.get(2).map_or("", |g| g.as_str())))
                    }
                }
            }))
            .await;
            Read {
                installed: pkgs.len(),
                packages: pkgs,
                list_title: PackageListTitle::Installed,
                updates: None,
                updates_note: format!(
                    "Binaries in {}; reinstall with `go install <module>@latest` to update.",
                    if gobin.is_empty() { "GOBIN" } else { &gobin }
                ),
            }
        }
        _ => Read { installed: 0, packages: vec![], list_title: PackageListTitle::Installed, updates: None, updates_note: String::new() },
    }
}

/// Installed apps plus updates whose current version comes from the installed list.
fn with_current(apps: Vec<PackageInfo>, updates: Option<Vec<PackageInfo>>, note: &str) -> Read {
    let current = versions(&apps);
    Read {
        installed: apps.len(),
        packages: apps,
        list_title: PackageListTitle::Installed,
        updates: updates
            .map(|u| u.into_iter().map(|u| upd(&u.name, current.get(&u.name).map_or("", String::as_str), Some(&u.version))).collect()),
        updates_note: note.into(),
    }
}

// ---------- detection and cache ----------

type Detected = Vec<(PackageManagerInfo, &'static str)>;

static DETECTED: Mutex<Option<(Instant, Detected)>> = Mutex::new(None);

fn detect() -> Detected {
    let mut cache = DETECTED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, list)) = cache.as_ref()
        && at.elapsed() < CACHE
    {
        return list.clone();
    }
    let list: Detected = MANAGERS
        .iter()
        .filter_map(|m| {
            let bin = m.bins.iter().find(|b| on_path(b))?;
            Some((PackageManagerInfo { id: m.id.into(), label: m.label.into(), kind: m.kind }, *bin))
        })
        .collect();
    *cache = Some((Instant::now(), list.clone()));
    list
}

pub fn package_managers() -> Vec<PackageManagerInfo> {
    detect().into_iter().map(|(m, _)| m).collect()
}

static DATA: LazyLock<Mutex<HashMap<String, (Instant, PackagesResponse)>>> = LazyLock::new(Default::default);

pub async fn manager_packages(id: &str, refresh: bool) -> ApiResult<PackagesResponse> {
    let Some((found, bin)) = detect().into_iter().find(|(m, _)| m.id == id) else {
        return Err(HttpError::not_found(format!("Package manager “{id}” not found on this server")));
    };
    if !refresh
        && let Some((at, data)) = DATA.lock().ok().and_then(|c| c.get(id).cloned())
        && at.elapsed() < CACHE
    {
        return Ok(data);
    }
    let r = read_manager(id, bin).await;
    let data = PackagesResponse {
        installed: r.installed,
        packages: r.packages,
        list_title: r.list_title,
        updates: r.updates,
        updates_note: r.updates_note,
        manager: id.into(),
        label: found.label,
        checked_at: crate::db::now(),
    };
    if let Ok(mut c) = DATA.lock() {
        c.insert(id.into(), (Instant::now(), data.clone()));
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_version_lines() {
        let list = name_version("linux 6.9.1-1\nfoo 1.0 -> 1.1\n\n");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].new_version, None);
        assert_eq!(list[1].new_version.as_deref(), Some("1.1"));
        let json = serde_json::to_string(&list[0]).unwrap();
        assert_eq!(json, r#"{"name":"linux","version":"6.9.1-1"}"#, "absent fields are dropped like undefined");
    }
}
