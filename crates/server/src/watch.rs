//! Live file changes of project folders — a port of `watch.ts` (Linux behavior), so open
//! editors, the file tree and git state follow what agents, shells and other people do.
//!
//! One watcher per folder, shared by every subscriber: started with the first one, stopped
//! after the last. Events are batched (`BATCH`) per folder. One non-recursive inotify watch
//! per directory, skipping heavy and generated folders (a recursive watch would walk
//! node_modules and can exhaust the inotify limit); new directories are picked up as they
//! appear. `.git` is not walked: changes of HEAD, index and refs are reported as `git`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::{broadcast, mpsc};

use crate::paths;

const BATCH: Duration = Duration::from_millis(150);
/// More changed paths than this in one batch → `overflow` (the client reloads everything).
const MAX_PATHS: usize = 200;
/// Directory watches per folder; past it deeper changes are not reported.
const MAX_DIRS: usize = 4000;
const SKIP: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".nuxt",
    ".venv",
    "venv",
    "__pycache__",
    ".cache",
    ".turbo",
    ".gradle",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    "coverage",
];

/// A batch of changes in one folder.
#[derive(Debug, Clone)]
pub struct Change {
    pub paths: Vec<String>,
    pub git: bool,
    pub overflow: bool,
}

fn is_git_file(rel: &str) -> bool {
    matches!(rel, "HEAD" | "ORIG_HEAD" | "FETCH_HEAD" | "MERGE_HEAD" | "index" | "packed-refs") || rel.starts_with("refs/")
}

type Slot = Arc<Mutex<Option<RecommendedWatcher>>>;

struct Folder {
    tx: broadcast::Sender<Change>,
    subscribers: usize,
    /// Shared with the batching task (it adds watches for new directories).
    watcher: Slot,
}

impl Drop for Folder {
    fn drop(&mut self) {
        // dropping the watcher drops its event sender: the batching task then ends
        self.watcher.lock().expect("watch lock").take();
    }
}

#[derive(Clone, Default)]
pub struct Watches {
    folders: Arc<Mutex<HashMap<PathBuf, Folder>>>,
}

/// An active subscription; dropping it unsubscribes (and stops the watcher after the last).
pub struct Subscription {
    watches: Watches,
    root: PathBuf,
    pub rx: broadcast::Receiver<Change>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let mut folders = self.watches.folders.lock().expect("watch lock");
        if let Some(f) = folders.get_mut(&self.root) {
            f.subscribers -= 1;
            if f.subscribers == 0 {
                folders.remove(&self.root);
            }
        }
    }
}

impl Watches {
    /// Subscribe to changes in a folder (absolute path).
    pub fn subscribe(&self, root: &Path) -> Subscription {
        let mut folders = self.folders.lock().expect("watch lock");
        if !folders.contains_key(root) {
            let (tx, _) = broadcast::channel(256);
            match start(root, tx.clone()) {
                Some(watcher) => {
                    folders.insert(root.to_path_buf(), Folder { tx, subscribers: 0, watcher });
                }
                // nothing can be watched: a subscription that never fires
                None => return Subscription { watches: self.clone(), root: PathBuf::new(), rx: tx.subscribe() },
            }
        }
        let f = folders.get_mut(root).expect("just inserted");
        f.subscribers += 1;
        Subscription { watches: self.clone(), root: root.to_path_buf(), rx: f.tx.subscribe() }
    }

    /// Number of folders being watched (for tests).
    pub fn watched(&self) -> usize {
        self.folders.lock().expect("watch lock").len()
    }
}

enum Raw {
    Path(PathBuf),
    Git,
}

fn start(root: &Path, tx: broadcast::Sender<Change>) -> Option<Slot> {
    let (raw_tx, raw_rx) = mpsc::unbounded_channel::<Raw>();
    let git_dir = root.join(".git");
    let dirs: Arc<Mutex<HashSet<PathBuf>>> = Arc::default();
    let handler = {
        let git_dir = git_dir.clone();
        move |res: notify::Result<Event>| {
            let Ok(ev) = res else { return };
            // reading a file (open / close without writing) is not a change; reporting it made
            // every client that reloads on change read the file again — an endless loop
            if matches!(ev.kind, notify::EventKind::Access(_)) {
                return;
            }
            for p in ev.paths {
                let msg = match p.strip_prefix(&git_dir) {
                    Ok(rel) if is_git_file(&paths::to_string(rel)) => Raw::Git,
                    Ok(_) => continue,
                    Err(_) => Raw::Path(p),
                };
                let _ = raw_tx.send(msg);
            }
        }
    };
    let mut watcher = notify::recommended_watcher(handler).ok()?;
    add_dirs(&mut watcher, root, &dirs);
    for d in [git_dir.clone(), git_dir.join("refs/heads"), git_dir.join("refs/remotes")] {
        if d.is_dir() {
            let _ = watcher.watch(&d, RecursiveMode::NonRecursive);
        }
    }
    let slot: Slot = Arc::new(Mutex::new(Some(watcher)));
    tokio::spawn(batch(root.to_path_buf(), raw_rx, tx, dirs, slot.clone()));
    Some(slot)
}

/// Non-recursive watches on `dir` and its subdirectories (skipping heavy folders).
fn add_dirs(w: &mut RecommendedWatcher, dir: &Path, dirs: &Mutex<HashSet<PathBuf>>) {
    {
        let mut set = dirs.lock().expect("watch lock");
        if set.len() >= MAX_DIRS || set.contains(dir) {
            return;
        }
        if w.watch(dir, RecursiveMode::NonRecursive).is_err() {
            return;
        }
        set.insert(dir.to_path_buf());
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for d in rd.flatten() {
        let name = d.file_name().to_string_lossy().into_owned();
        if d.file_type().is_ok_and(|t| t.is_dir()) && !SKIP.contains(&name.as_str()) {
            add_dirs(w, &dir.join(name), dirs);
        }
    }
}

async fn batch(
    root: PathBuf,
    mut rx: mpsc::UnboundedReceiver<Raw>,
    tx: broadcast::Sender<Change>,
    dirs: Arc<Mutex<HashSet<PathBuf>>>,
    watcher: Slot,
) {
    while let Some(first) = rx.recv().await {
        let mut set: Vec<String> = vec![];
        let mut seen = HashSet::new();
        let (mut git, mut overflow) = (false, false);
        let mut take = |r: Raw, set: &mut Vec<String>, git: &mut bool, overflow: &mut bool| match r {
            Raw::Git => *git = true,
            Raw::Path(p) => {
                let Ok(rel) = p.strip_prefix(&root) else { return };
                let rel = paths::to_string(rel);
                if rel.is_empty() {
                    return;
                }
                // a new directory gets its own watch (unless it is a skipped one)
                let name = paths::basename(&p);
                if p.is_dir() && !SKIP.contains(&name.as_str()) && !dirs.lock().expect("watch lock").contains(&p) {
                    let mut slot = watcher.lock().expect("watch lock");
                    if let Some(w) = slot.as_mut() {
                        add_dirs(w, &p, &dirs);
                    }
                }
                if seen.insert(rel.clone()) {
                    if set.len() >= MAX_PATHS {
                        *overflow = true;
                    } else {
                        set.push(rel);
                    }
                }
            }
        };
        take(first, &mut set, &mut git, &mut overflow);
        let deadline = tokio::time::Instant::now() + BATCH;
        while let Ok(Some(r)) = tokio::time::timeout_at(deadline, rx.recv()).await {
            take(r, &mut set, &mut git, &mut overflow);
        }
        if set.is_empty() && !git && !overflow {
            continue;
        }
        let msg = if overflow { Change { paths: vec![], git: true, overflow: true } } else { Change { paths: set, git, overflow: false } };
        let _ = tx.send(msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn one_watcher_per_folder_stopped_after_the_last_subscriber() {
        let dir = tempfile::tempdir().unwrap();
        let w = Watches::default();
        let mut a = w.subscribe(dir.path());
        let b = w.subscribe(dir.path());
        assert_eq!(w.watched(), 1);
        std::fs::write(dir.path().join("f.txt"), "x").unwrap();
        let change = tokio::time::timeout(Duration::from_secs(5), a.rx.recv()).await.unwrap().unwrap();
        assert_eq!(change.paths, vec!["f.txt"]);
        drop(b);
        assert_eq!(w.watched(), 1);
        drop(a);
        assert_eq!(w.watched(), 0);
    }

    #[test]
    fn git_files() {
        assert!(is_git_file("HEAD") && is_git_file("index") && is_git_file("refs/heads/main"));
        assert!(!is_git_file("objects/ab/cd") && !is_git_file("config"));
    }
}
