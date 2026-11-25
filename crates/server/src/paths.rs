//! Path helpers with the semantics of Node's `path` module where the API depends on it.

use std::path::{Component, Path, PathBuf};

/// Like Node's `path.resolve(p)`: absolute against the current folder, `.` and `..`
/// removed lexically (symlinks are not resolved).
pub fn resolve(p: &str) -> PathBuf {
    let path = Path::new(p);
    let joined =
        if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")).join(path) };
    normalize(&joined)
}

/// Remove `.` and `..` components without touching the file system.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() { PathBuf::from("/") } else { out }
}

/// Last path component as a string (`basename`).
pub fn basename(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

pub fn to_string(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// The user's home folder (`$HOME`; `/` when it is unset or empty).
pub fn home() -> PathBuf {
    std::env::var_os("HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_like_node() {
        assert_eq!(normalize(Path::new("/a/./b/../c/")), PathBuf::from("/a/c"));
        assert_eq!(normalize(Path::new("/../..")), PathBuf::from("/"));
        assert_eq!(resolve("/x/y/.."), PathBuf::from("/x"));
        assert!(resolve("rel").is_absolute());
    }
}
