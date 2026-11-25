//! Running official CLIs for System pages (C locale, no colors, with a timeout).
//! Arguments go straight to the program (no shell).

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::Command;

/// Default timeout of a command.
pub const TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Default)]
pub struct CmdResult {
    pub code: i32,
    pub out: String,
    pub err: String,
}

impl CmdResult {
    /// stdout + stderr, trimmed (what the UI shows for an action).
    pub fn output(&self) -> String {
        format!("{}{}", self.out, self.err).trim().to_string()
    }
}

/// Run `cmd args…` with the default timeout.
pub async fn sh(cmd: &str, args: &[&str]) -> CmdResult {
    sh_timeout(cmd, args, TIMEOUT).await
}

/// Run `cmd args…`: a missing binary is code 127, a killed or timed-out process code 1
/// (with the output read so far).
pub async fn sh_timeout(cmd: &str, args: &[&str], timeout: Duration) -> CmdResult {
    let mut c = Command::new(cmd);
    c.args(args)
        .env_clear()
        .envs(crate::config::child_env())
        .env("LC_ALL", "C")
        .env("SYSTEMD_COLORS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = match c.spawn() {
        Ok(ch) => ch,
        Err(e) => return CmdResult { code: 127, out: String::new(), err: e.to_string() },
    };
    let read = |pipe: Option<Box<dyn tokio::io::AsyncRead + Unpin + Send>>| {
        tokio::spawn(async move {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut buf).await;
            }
            String::from_utf8_lossy(&buf).into_owned()
        })
    };
    let out = read(child.stdout.take().map(|p| Box::new(p) as _));
    let err = read(child.stderr.take().map(|p| Box::new(p) as _));
    let code = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => status.code().unwrap_or(1),
        Ok(Err(_)) => 1,
        Err(_) => {
            let _ = child.kill().await;
            1
        }
    };
    // a grandchild may keep a pipe open: don't wait for it forever
    let collect = |h: tokio::task::JoinHandle<String>| async move {
        tokio::time::timeout(Duration::from_secs(2), h).await.ok().and_then(Result::ok).unwrap_or_default()
    };
    CmdResult { code, out: collect(out).await, err: collect(err).await }
}

/// Whether `bin` is a file in one of the PATH folders.
pub fn on_path(bin: &str) -> bool {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path).any(|dir| !dir.as_os_str().is_empty() && Path::new(&dir).join(bin).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runs_without_a_shell() {
        let r = sh("printf", &["%s", "$HOME; echo hi"]).await;
        assert_eq!(r.code, 0);
        assert_eq!(r.out, "$HOME; echo hi");
        assert_eq!(sh("definitely-not-a-command-xyz", &[]).await.code, 127);
        let slow = sh_timeout("sleep", &["5"], Duration::from_millis(100)).await;
        assert_eq!(slow.code, 1);
        assert!(on_path("sh"));
        assert!(!on_path("definitely-not-a-command-xyz"));
    }
}
