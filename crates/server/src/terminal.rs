//! Terminal tasks: installing, logging in, updating harnesses, project Run and Shell
//! (a port of `terminal.ts` and the `/api/jobs` routes of `routes/harness.ts`).
//!
//! The command runs on a real pty, so interactive CLIs (login via link with a code, picking
//! a provider with arrow keys) work just like in a terminal. The browser renders output in
//! xterm.js and sends keystrokes back.
//!
//! The TS server gets its pty from util-linux `script`. Here the pty is opened directly with
//! `openpty(3)` from `libc` (already a dependency): no extra crate, no `script` binary, and
//! the window size is set with an ioctl instead of `stty`. The command runs as
//! `/bin/bash -c <command>` in its own session (`setsid`) with the pty as the controlling
//! terminal, which is what `script` does too.

use std::collections::HashMap;
use std::convert::Infallible;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, Stdio};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::{Path, State};
use axum::response::sse::{Event, Sse};
use axum::routing::{get, post};
use futures_util::stream::{self, Stream, StreamExt};
use hubness_core::{JobInfo, JobStatus, JobStreamMsg};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::error::{ApiResult, HttpError};
use crate::http::JsonBody;
use crate::{AppState, paths};

/// Output kept for a late subscriber (bytes; the TS server counts UTF-16 units).
const OUTPUT_LIMIT: usize = 512 * 1024;
const KEEP_AFTER_EXIT: Duration = Duration::from_secs(15 * 60);
/// Keep-alive period of the SSE stream (as in the TS server).
const PING: Duration = Duration::from_secs(25);

/// Escaping for sh: 'a'\''b'.
pub fn shq(s: &str) -> String {
    let plain = !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&b));
    if plain { s.to_string() } else { format!("'{}'", s.replace('\'', r"'\''")) }
}

pub type Callback = Box<dyn Fn() + Send + Sync>;
pub type OutputFn = Box<dyn Fn(&str) + Send + Sync>;
pub type ExitFn = Box<dyn Fn(Option<i32>) + Send + Sync>;

/// How to start a terminal task.
#[derive(Default)]
pub struct JobOpts {
    pub title: String,
    pub command: String,
    /// What to show instead of the command (sandbox/container wrappers are long).
    pub display: Option<String>,
    pub env: Vec<(String, String)>,
    pub cwd: Option<String>,
    pub cols: Option<f64>,
    pub rows: Option<f64>,
    pub on_output: Option<OutputFn>,
    pub on_exit: Option<ExitFn>,
    /// Extra stop step (a process in the container doesn't die with the docker exec client).
    pub on_kill: Option<Callback>,
}

struct Job {
    info: JobInfo,
    pid: i32,
    output: String,
    listeners: Vec<mpsc::UnboundedSender<JobStreamMsg>>,
    stopped: bool,
    exited_at: Option<Instant>,
    on_kill: Option<Callback>,
    /// The pty master for input; `None` once the task exited.
    input: Option<File>,
}

impl Job {
    fn send(&mut self, msg: JobStreamMsg) {
        self.listeners.retain(|l| l.send(msg.clone()).is_ok());
    }
}

type JobRef = Arc<Mutex<Job>>;

static JOBS: LazyLock<Mutex<HashMap<String, JobRef>>> = LazyLock::new(Default::default);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The job, dropping tasks that exited more than 15 minutes ago.
fn job(id: &str) -> Option<JobRef> {
    let mut jobs = lock(&JOBS);
    jobs.retain(|_, j| lock(j).exited_at.is_none_or(|t| t.elapsed() < KEEP_AFTER_EXIT));
    jobs.get(id).cloned()
}

/// A pty pair of the given size; both ends close on exec (stdio gets dup2'ed copies).
fn open_pty(cols: u16, rows: u16) -> std::io::Result<(OwnedFd, OwnedFd)> {
    let mut master = -1;
    let mut slave = -1;
    let size = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: openpty writes two fds into the out pointers; name/termios may be null
    let rc = unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null(), &size) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: both fds were just returned by openpty and are owned by nobody else
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    for fd in [&master, &slave] {
        // SAFETY: plain fcntl on a valid fd
        unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    Ok((master, slave))
}

fn now_iso() -> String {
    crate::db::now()
}

/// Start a terminal task on a pty.
pub fn start_job(opts: JobOpts) -> JobInfo {
    let cols = opts.cols.unwrap_or(100.0).round().clamp(40.0, 240.0) as u16;
    let rows = opts.rows.unwrap_or(30.0).round().clamp(10.0, 80.0) as u16;
    let info = JobInfo {
        id: crate::db::new_id(),
        title: opts.title.clone(),
        command: opts.display.clone().unwrap_or_else(|| opts.command.clone()),
        status: JobStatus::Running,
        exit_code: None,
        started_at: now_iso(),
    };
    let job = Arc::new(Mutex::new(Job {
        info: info.clone(),
        pid: 0,
        output: String::new(),
        listeners: vec![],
        stopped: false,
        exited_at: None,
        on_kill: opts.on_kill,
        input: None,
    }));
    lock(&JOBS).insert(info.id.clone(), job.clone());
    let on_output = opts.on_output;
    let on_exit = opts.on_exit;

    let spawned = open_pty(cols, rows).and_then(|(master, slave)| {
        let mut cmd = Command::new("/bin/bash");
        cmd.arg("-c")
            .arg(&opts.command)
            .current_dir(opts.cwd.clone().map(std::path::PathBuf::from).unwrap_or_else(paths::home))
            .env_clear();
        cmd.envs(crate::config::child_env()).envs(opts.env.iter().map(|(k, v)| (k, v)));
        // a POSIX shell, not the user's fish/zsh, for scripts that use $SHELL
        cmd.env("SHELL", "/bin/bash").env("TERM", "xterm-256color").env("COLORTERM", "truecolor");
        cmd.stdin(Stdio::from(slave.try_clone()?)).stdout(Stdio::from(slave.try_clone()?)).stderr(Stdio::from(slave));
        // SAFETY: setsid and ioctl are async-signal-safe; nothing else runs between fork and exec
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // the pty (stdin) becomes the controlling terminal of the new session
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = cmd.spawn()?;
        drop(cmd); // closes our copies of the slave: reads end with EIO once the task is gone
        Ok((child, File::from(master)))
    });
    let (mut child, master) = match spawned {
        Ok(v) => v,
        Err(e) => {
            push(&job, &format!("\r\n[hub] failed to start: {e}\r\n"), on_output.as_deref(), None);
            finish(&job, Some(-2), on_exit.as_deref());
            return lock(&job).info.clone();
        }
    };
    let writer = master.try_clone().ok();
    {
        let mut j = lock(&job);
        j.pid = child.id() as i32;
        j.input = writer;
    }

    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let reader_job = job.clone();
    std::thread::spawn(move || {
        let mut master = master;
        let mut answer = master.try_clone().ok();
        let mut buf = vec![0u8; 16 * 1024];
        let mut pending: Vec<u8> = vec![];
        let mut tail = String::new();
        loop {
            match master.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    pending.extend_from_slice(&buf[..n]);
                    let text = take_utf8(&mut pending);
                    if !text.is_empty() {
                        push(&reader_job, &text, on_output.as_deref(), Some((&mut tail, &mut answer)));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break, // EIO: every slave end is closed
            }
        }
        if !pending.is_empty() {
            push(&reader_job, &String::from_utf8_lossy(&pending), on_output.as_deref(), None);
        }
        let _ = done_tx.send(());
    });

    let waiter_job = job.clone();
    std::thread::spawn(move || {
        let status = child.wait().ok();
        // let the reader drain what the task printed last (a background process that keeps
        // the pty open must not hold the exit back)
        let _ = done_rx.recv_timeout(Duration::from_millis(500));
        let code = status.map(|s| s.code().unwrap_or_else(|| 128 + s.signal().unwrap_or(0)));
        finish(&waiter_job, code, on_exit.as_deref());
    });
    info
}

/// Decode the complete UTF-8 prefix of `buf`, keeping an incomplete trailing sequence.
fn take_utf8(buf: &mut Vec<u8>) -> String {
    match std::str::from_utf8(buf) {
        Ok(s) => {
            let s = s.to_string();
            buf.clear();
            s
        }
        Err(e) if e.error_len().is_none() => {
            let valid = e.valid_up_to();
            let s = String::from_utf8_lossy(&buf[..valid]).into_owned();
            buf.drain(..valid);
            s
        }
        Err(_) => {
            let s = String::from_utf8_lossy(buf).into_owned();
            buf.clear();
            s
        }
    }
}

/// Count Primary Device Attributes queries (ESC [ c / ESC [ 0 c) that end in the new chunk.
fn device_queries(prev: &str, data: &str) -> usize {
    let text = format!("{prev}{data}");
    let mut n = 0;
    for pat in ["\x1b[c", "\x1b[0c"] {
        n += text.match_indices(pat).filter(|(i, m)| i + m.len() > prev.len()).count();
    }
    n
}

fn push(job: &JobRef, data: &str, on_output: Option<&(dyn Fn(&str) + Send + Sync)>, answer: Option<(&mut String, &mut Option<File>)>) {
    // Primary Device Attributes query: answered here, at once, the way xterm.js would —
    // shells like fish 4 wait up to 10 s for it at start, often before any browser is
    // attached. The browser terminal drops its own reply (see Terminal.tsx).
    if let Some((tail, Some(w))) = answer {
        for _ in 0..device_queries(tail, data) {
            let _ = w.write_all(b"\x1b[?1;2c");
        }
        let joined = format!("{tail}{data}");
        let chars: Vec<char> = joined.chars().collect();
        *tail = chars[chars.len().saturating_sub(3)..].iter().collect();
    }
    {
        let mut j = lock(job);
        j.output.push_str(data);
        if j.output.len() > OUTPUT_LIMIT {
            let mut cut = j.output.len() - OUTPUT_LIMIT;
            while !j.output.is_char_boundary(cut) {
                cut += 1;
            }
            j.output.drain(..cut);
        }
        j.send(JobStreamMsg::Output { data: data.to_string() });
    }
    if let Some(f) = on_output {
        f(data);
    }
}

fn finish(job: &JobRef, exit: Option<i32>, on_exit: Option<&(dyn Fn(Option<i32>) + Send + Sync)>) {
    let code = {
        let mut j = lock(job);
        // a task stopped by the user has no exit code
        let code = if j.stopped { None } else { exit };
        j.info.status = JobStatus::Exited;
        j.info.exit_code = code;
        j.exited_at = Some(Instant::now());
        j.input = None;
        code
    };
    // reset caches first (on_exit), then notify the client — otherwise it re-reads stale data
    if let Some(f) = on_exit {
        f(code);
    }
    let mut j = lock(job);
    j.send(JobStreamMsg::Exit { code });
    j.listeners.clear();
}

/// Number of attached streams (shells nobody watches are closed after a while).
pub fn job_listeners(id: &str) -> usize {
    job(id).map(|j| lock(&j).listeners.iter().filter(|l| !l.is_closed()).count()).unwrap_or(0)
}

pub fn get_job(id: &str) -> Option<(JobInfo, String)> {
    job(id).map(|j| {
        let j = lock(&j);
        (j.info.clone(), j.output.clone())
    })
}

pub fn job_info(id: &str) -> Option<JobInfo> {
    job(id).map(|j| lock(&j).info.clone())
}

/// The snapshot and a subscription, taken atomically (nothing printed in between is lost).
fn subscribe(id: &str) -> Option<(JobInfo, String, mpsc::UnboundedReceiver<JobStreamMsg>)> {
    let j = job(id)?;
    let mut j = lock(&j);
    let (tx, rx) = mpsc::unbounded_channel();
    // an exited task keeps the stream open too (pings), or EventSource would reconnect in a loop
    j.listeners.push(tx);
    Some((j.info.clone(), j.output.clone(), rx))
}

pub fn write_job(id: &str, data: &str) -> bool {
    let Some(j) = job(id) else { return false };
    let mut j = lock(&j);
    if j.info.status != JobStatus::Running {
        return false;
    }
    match j.input.as_mut() {
        Some(w) => w.write_all(data.as_bytes()).is_ok(),
        None => false,
    }
}

/// Process children (from /proc).
fn child_pids(pid: i32) -> Vec<i32> {
    std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children"))
        .map(|s| s.split_whitespace().filter_map(|p| p.parse().ok()).collect())
        .unwrap_or_default()
}

/// Stop the whole task: its session's group and its children's groups (job-control shells
/// put commands into groups of their own). SIGHUP — like closing a terminal window:
/// interactive shells ignore SIGTERM but exit immediately on SIGHUP; then SIGTERM; after
/// 3 s — SIGKILL.
fn kill_tree(pid: i32) {
    if pid <= 0 {
        return;
    }
    let mut groups = vec![pid];
    groups.extend(child_pids(pid));
    let signal = move |groups: &[i32], sig: i32| {
        for g in groups {
            // SAFETY: kill(2) with a negative pid signals a process group
            unsafe { libc::kill(-g, sig) };
        }
    };
    signal(&groups, libc::SIGHUP);
    signal(&groups, libc::SIGTERM);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(3));
        signal(&groups, libc::SIGKILL);
    });
}

pub fn kill_job(id: &str) -> bool {
    let Some(j) = job(id) else { return false };
    let (pid, on_kill) = {
        let mut j = lock(&j);
        if j.info.status != JobStatus::Running {
            return false;
        }
        j.stopped = true;
        (j.pid, j.on_kill.take())
    };
    if let Some(f) = on_kill {
        f();
    }
    kill_tree(pid);
    true
}

/// On hub exit: stop all tasks (dev servers, shells) so they don't become orphans.
pub fn kill_all_jobs() {
    let ids: Vec<String> = lock(&JOBS).keys().cloned().collect();
    for id in ids {
        kill_job(&id);
    }
}

// ---------- routes ----------

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/jobs/{id}/stream", get(stream_route))
        .route("/jobs/{id}/input", post(input_route))
        .route("/jobs/{id}/kill", post(kill_route))
}

fn sse(m: &JobStreamMsg) -> Result<Event, Infallible> {
    Ok(Event::default().data(serde_json::to_string(m).unwrap_or_default()))
}

/// The job's output: a snapshot first, then output chunks until the exit message.
async fn stream_route(Path(id): Path<String>) -> ApiResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let (job, output, rx) = subscribe(&id).ok_or_else(|| HttpError::not_found("Job not found"))?;
    let first = JobStreamMsg::Snapshot { job, output };
    let ticks = tokio::time::interval_at(tokio::time::Instant::now() + PING, PING);
    let rest = stream::unfold((rx, ticks, false), |(mut rx, mut ticks, done)| async move {
        if done {
            return None;
        }
        tokio::select! {
            m = rx.recv() => {
                // the sender is dropped once the task exited and the exit was sent
                let m = m?;
                let last = matches!(m, JobStreamMsg::Exit { .. });
                Some((sse(&m), (rx, ticks, last)))
            }
            _ = ticks.tick() => Some((Ok(Event::default().event("ping").data("")), (rx, ticks, false))),
        }
    });
    Ok(Sse::new(stream::once(async move { sse(&first) }).chain(rest)))
}

#[derive(Deserialize)]
struct InputBody {
    data: String,
}

async fn input_route(Path(id): Path<String>, JsonBody(b): JsonBody<InputBody>) -> ApiResult<axum::Json<Value>> {
    if b.data.encode_utf16().count() > 4096 {
        return Err(HttpError::bad_request("data: at most 4096 characters"));
    }
    if !write_job(&id, &b.data) {
        return Err(HttpError::new(409, "The command is not running"));
    }
    Ok(axum::Json(json!({ "ok": true })))
}

async fn kill_route(State(_): State<AppState>, Path(id): Path<String>) -> axum::Json<Value> {
    axum::Json(json!({ "ok": kill_job(&id) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_for(id: &str, f: impl Fn(&JobInfo, &str) -> bool) -> (JobInfo, String) {
        for _ in 0..100 {
            let (info, out) = get_job(id).unwrap();
            if f(&info, &out) {
                return (info, out);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        get_job(id).unwrap()
    }

    #[test]
    fn quoting_for_sh() {
        assert_eq!(shq("a-b/c.d"), "a-b/c.d");
        assert_eq!(shq("it's"), r"'it'\''s'");
        assert_eq!(shq(""), "''");
        assert_eq!(shq("a b"), "'a b'");
    }

    #[test]
    fn runs_on_a_pty_with_exit_code_and_size() {
        let info = start_job(JobOpts {
            title: "t".into(),
            command: "test -t 0 && stty size; printf 'é\\n'; exit 3".into(),
            cols: Some(120.0),
            rows: Some(40.0),
            ..Default::default()
        });
        let (info2, out) = wait_for(&info.id, |i, _| i.status == JobStatus::Exited);
        assert_eq!(info2.exit_code, Some(3));
        assert!(out.contains("40 120"), "{out:?}");
        assert!(out.contains('é'));
    }

    #[test]
    fn answers_primary_device_attributes() {
        let info = start_job(JobOpts {
            title: "da1".into(),
            command: r#"printf '\033[0c'; IFS= read -r -s -d c -t 5 r; printf 'REPLY:%s:END' "${r#?}""#.into(),
            ..Default::default()
        });
        let (_, out) = wait_for(&info.id, |_, o| o.contains("END"));
        kill_job(&info.id);
        assert!(out.contains("REPLY:[?1;2:END"), "{out:?}");
        assert_eq!(device_queries("\x1b[", "0c"), 1, "a query split across chunks is seen once");
        assert_eq!(device_queries("\x1b[0c", "x"), 0);
    }

    #[test]
    fn input_and_kill() {
        let info = start_job(JobOpts { title: "k".into(), command: "read -r l; echo got:$l; sleep 30".into(), ..Default::default() });
        assert!(write_job(&info.id, "hi\n"));
        wait_for(&info.id, |_, o| o.contains("got:hi"));
        assert!(kill_job(&info.id));
        let (done, _) = wait_for(&info.id, |i, _| i.status == JobStatus::Exited);
        assert_eq!(done.exit_code, None, "a stopped task has no exit code");
        assert!(!kill_job(&info.id) && !write_job(&info.id, "x"));
    }
}
