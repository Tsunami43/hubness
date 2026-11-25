//! User accounts: storage, passwords (argon2id), sign-in sessions and the `hubness user …`
//! commands that manage accounts directly in the database of `HUBNESS_DATA_DIR`.
//!
//! All accounts have the same rights. A session token is random (256 bits) and handed to the
//! client once; the database keeps only its SHA-256, so a copy of the database can't be used
//! to sign in.

use std::io::{BufRead, Write};
use std::sync::OnceLock;

use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use hubness_core::User;
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

use crate::db;
use crate::error::{ApiResult, HttpError};

pub const MIN_PASSWORD: usize = 8;
const MAX_PASSWORD: usize = 1024;
const MAX_USERNAME: usize = 64;
/// How long a session lives without being used (each use extends it).
const SESSION_DAYS: i64 = 30;
/// `last_seen` / `expires` are written at most this often per session.
const TOUCH_SECS: i64 = 300;

/// A username as stored: trimmed; letters, digits, `.`, `_`, `-` (1–64 characters).
/// Lookups ignore case.
pub fn check_username(name: &str) -> ApiResult<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(HttpError::bad_request("username: enter a username"));
    }
    if name.chars().count() > MAX_USERNAME {
        return Err(HttpError::bad_request(format!("username: at most {MAX_USERNAME} characters")));
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) {
        return Err(HttpError::bad_request("username: only latin letters, digits, dots, dashes and underscores"));
    }
    Ok(name.to_string())
}

pub fn check_password(password: &str) -> ApiResult<()> {
    let n = password.chars().count();
    if n < MIN_PASSWORD {
        return Err(HttpError::bad_request(format!("password: at least {MIN_PASSWORD} characters")));
    }
    if n > MAX_PASSWORD {
        return Err(HttpError::bad_request(format!("password: at most {MAX_PASSWORD} characters")));
    }
    Ok(())
}

/// An argon2id PHC string (`$argon2id$v=19$…`) with a random salt. CPU-heavy: call it off
/// the async threads.
pub fn hash_password(password: &str) -> ApiResult<String> {
    argon2::Argon2::default()
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| HttpError::new(500, format!("Password hashing failed: {e}")))
}

/// Does `password` match the stored hash? `None` (no such user) still runs a full
/// verification against a dummy hash, so the answer takes the same time either way.
pub fn verify_password(password: &str, hash: Option<&str>) -> bool {
    static DUMMY: OnceLock<String> = OnceLock::new();
    let dummy = DUMMY.get_or_init(|| hash_password("hubness-dummy-password").unwrap_or_default());
    let ok = argon2::Argon2::default().verify_password(password.as_bytes(), hash.unwrap_or(dummy)).is_ok();
    ok && hash.is_some()
}

fn user_row(r: &rusqlite::Row) -> rusqlite::Result<User> {
    Ok(User { id: r.get("id")?, username: r.get("username")?, created_at: r.get("created_at")? })
}

pub fn count(db: &Connection) -> rusqlite::Result<i64> {
    db.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
}

pub fn list(db: &Connection) -> rusqlite::Result<Vec<User>> {
    let mut st = db.prepare("SELECT id, username, created_at FROM users ORDER BY username COLLATE NOCASE")?;
    st.query_map([], user_row)?.collect()
}

pub fn get(db: &Connection, id: &str) -> rusqlite::Result<Option<User>> {
    db.query_row("SELECT id, username, created_at FROM users WHERE id = ?", [id], user_row).optional()
}

/// The user (case-insensitive name) with the password hash.
pub fn find(db: &Connection, username: &str) -> rusqlite::Result<Option<(User, String)>> {
    db.query_row("SELECT id, username, created_at, password_hash FROM users WHERE username = ? COLLATE NOCASE", [username.trim()], |r| {
        Ok((user_row(r)?, r.get("password_hash")?))
    })
    .optional()
}

pub fn password_hash(db: &Connection, id: &str) -> rusqlite::Result<Option<String>> {
    db.query_row("SELECT password_hash FROM users WHERE id = ?", [id], |r| r.get(0)).optional()
}

/// Add an account (`username` already checked, `hash` from `hash_password`).
pub fn create(db: &Connection, username: &str, hash: &str) -> ApiResult<User> {
    if find(db, username)?.is_some() {
        return Err(HttpError::new(409, format!("The username {username} is taken")));
    }
    let user = User { id: db::new_id(), username: username.to_string(), created_at: db::now() };
    db.execute(
        "INSERT INTO users (id, username, password_hash, created_at) VALUES (?, ?, ?, ?)",
        params![user.id, user.username, hash, user.created_at],
    )?;
    Ok(user)
}

/// Add the very first account; refused once any account exists (two first-run forms racing).
pub fn create_first(db: &Connection, username: &str, hash: &str) -> ApiResult<User> {
    db::tx(db, || {
        if count(db)? > 0 {
            return Err(HttpError::new(409, "The hub already has accounts: sign in"));
        }
        create(db, username, hash)
    })
}

/// Set a new password and end the user's sessions (except `keep`, the session that changed it).
pub fn set_password(db: &Connection, id: &str, hash: &str, keep: Option<&str>) -> ApiResult<()> {
    db::tx(db, || {
        if db.execute("UPDATE users SET password_hash = ? WHERE id = ?", params![hash, id])? == 0 {
            return Err(HttpError::not_found("No such user"));
        }
        let keep = keep.map(token_hash).unwrap_or_default();
        db.execute("DELETE FROM auth_sessions WHERE user_id = ? AND token_hash != ?", params![id, keep])?;
        Ok(())
    })
}

/// Delete an account with its sessions; the last account can't be deleted (that would open
/// the hub again).
pub fn delete(db: &Connection, id: &str) -> ApiResult<User> {
    db::tx(db, || {
        let Some(user) = get(db, id)? else { return Err(HttpError::not_found("No such user")) };
        if count(db)? <= 1 {
            return Err(HttpError::new(409, "The last account can't be deleted: the hub would be open to anyone"));
        }
        db.execute("DELETE FROM auth_sessions WHERE user_id = ?", [id])?;
        db.execute("DELETE FROM users WHERE id = ?", [id])?;
        Ok(user)
    })
}

// ---------- sessions ----------

/// 256 random bits, URL-safe base64.
pub fn random_token() -> std::io::Result<String> {
    use base64::Engine;
    let mut buf = [0u8; 32];
    std::io::Read::read_exact(&mut std::fs::File::open("/dev/urandom")?, &mut buf)?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf))
}

pub fn token_hash(token: &str) -> String {
    Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn stamp(t: jiff::Timestamp) -> String {
    crate::db::iso(t)
}

/// Start a session for the user; returns the token (shown to the client once).
pub fn create_session(db: &Connection, user_id: &str) -> ApiResult<String> {
    let token = random_token()?;
    let now = jiff::Timestamp::now();
    db.execute("DELETE FROM auth_sessions WHERE expires < ?", [stamp(now)])?;
    db.execute(
        "INSERT INTO auth_sessions (token_hash, user_id, created_at, last_seen, expires) VALUES (?, ?, ?, ?, ?)",
        params![token_hash(&token), user_id, stamp(now), stamp(now), stamp(now + jiff::SignedDuration::from_hours(SESSION_DAYS * 24))],
    )?;
    Ok(token)
}

/// The user of a live session (extending it), `None` for unknown or expired tokens.
pub fn session_user(db: &Connection, token: &str) -> rusqlite::Result<Option<User>> {
    let hash = token_hash(token);
    let row = db
        .query_row(
            "SELECT u.id, u.username, u.created_at, s.last_seen, s.expires FROM auth_sessions s JOIN users u ON u.id = s.user_id
             WHERE s.token_hash = ?",
            [&hash],
            |r| Ok((user_row(r)?, r.get::<_, String>("last_seen")?, r.get::<_, String>("expires")?)),
        )
        .optional()?;
    let Some((user, last_seen, expires)) = row else { return Ok(None) };
    let now = jiff::Timestamp::now();
    if expires < stamp(now) {
        db.execute("DELETE FROM auth_sessions WHERE token_hash = ?", [&hash])?;
        return Ok(None);
    }
    if last_seen < stamp(now - jiff::SignedDuration::from_secs(TOUCH_SECS)) {
        db.execute(
            "UPDATE auth_sessions SET last_seen = ?, expires = ? WHERE token_hash = ?",
            params![stamp(now), stamp(now + jiff::SignedDuration::from_hours(SESSION_DAYS * 24)), hash],
        )?;
    }
    Ok(Some(user))
}

pub fn delete_session(db: &Connection, token: &str) -> rusqlite::Result<()> {
    db.execute("DELETE FROM auth_sessions WHERE token_hash = ?", [token_hash(token)]).map(|_| ())
}

// ---------- hubness user … ----------

const USAGE: &str = "usage: hubness user add <name> | passwd <name> | rm <name> | list
  The password is asked for (hidden) or read from stdin when it is not a terminal.
  Works on the database in HUBNESS_DATA_DIR (default .data), also while the hub runs.";

/// `hubness user <args…>`: manage accounts directly in the hub database.
pub fn cli(args: &[String]) -> anyhow::Result<()> {
    let config = crate::Config::from_env()?;
    config.ensure_dirs()?;
    let db = db::open(&config.db_path)?;
    let err = |e: HttpError| anyhow::anyhow!(e.message);
    let one = |what: &str| -> anyhow::Result<String> {
        match args.get(1) {
            Some(n) if args.len() == 2 => Ok(n.clone()),
            _ => anyhow::bail!("usage: hubness user {what} <name>"),
        }
    };
    match args.first().map(String::as_str) {
        Some("add") => {
            let name = check_username(&one("add")?).map_err(err)?;
            if find(&db, &name)?.is_some() {
                anyhow::bail!("the username {name} is taken");
            }
            let password = read_password(&format!("Password for {name}"))?;
            let user = create(&db, &name, &hash_password(&password).map_err(err)?).map_err(err)?;
            println!("added {}", user.username);
        }
        Some("passwd") => {
            let name = one("passwd")?;
            let Some((user, _)) = find(&db, &name)? else { anyhow::bail!("no user {name}") };
            let password = read_password(&format!("New password for {}", user.username))?;
            set_password(&db, &user.id, &hash_password(&password).map_err(err)?, None).map_err(err)?;
            println!("password changed for {} (signed out everywhere)", user.username);
        }
        Some("rm") => {
            let name = one("rm")?;
            let Some((user, _)) = find(&db, &name)? else { anyhow::bail!("no user {name}") };
            delete(&db, &user.id).map_err(|e| anyhow::anyhow!("{} (reset its password with `hubness user passwd {name}`)", e.message))?;
            println!("removed {}", user.username);
        }
        Some("list") if args.len() == 1 => {
            let users = list(&db)?;
            if users.is_empty() {
                println!("no accounts: the hub is open to this machine only (add one with `hubness user add <name>`)");
            }
            for u in users {
                println!("{}\t{}", u.username, u.created_at);
            }
        }
        _ => anyhow::bail!("{USAGE}"),
    }
    Ok(())
}

/// A new password: typed twice with echo off on a terminal, else one line from stdin.
fn read_password(prompt: &str) -> anyhow::Result<String> {
    let stdin = std::io::stdin();
    // SAFETY: isatty only inspects the descriptor
    if unsafe { libc::isatty(0) } != 1 {
        let mut line = String::new();
        stdin.lock().read_line(&mut line)?;
        let password = line.trim_end_matches(['\n', '\r']).to_string();
        check_password(&password).map_err(|e| anyhow::anyhow!(e.message))?;
        return Ok(password);
    }
    let first = read_hidden(&format!("{prompt}: "))?;
    check_password(&first).map_err(|e| anyhow::anyhow!(e.message))?;
    if read_hidden("Repeat it: ")? != first {
        anyhow::bail!("the passwords don't match");
    }
    Ok(first)
}

/// Read a line from the terminal without echoing it.
fn read_hidden(prompt: &str) -> anyhow::Result<String> {
    let mut err = std::io::stderr();
    write!(err, "{prompt}")?;
    err.flush()?;
    // SAFETY: plain termios calls on stdin with a zeroed struct filled by tcgetattr
    let saved = unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(0, &mut t) != 0 {
            anyhow::bail!("can't read the terminal settings");
        }
        let saved = t;
        t.c_lflag &= !libc::ECHO;
        t.c_lflag |= libc::ECHONL;
        libc::tcsetattr(0, libc::TCSANOW, &t);
        saved
    };
    let mut line = String::new();
    let read = std::io::stdin().lock().read_line(&mut line);
    // SAFETY: restores the settings read above
    unsafe {
        libc::tcsetattr(0, libc::TCSANOW, &saved);
    }
    read?;
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db::init(&db).unwrap();
        db
    }

    #[test]
    fn names_and_passwords() {
        assert_eq!(check_username("  alice ").unwrap(), "alice");
        assert!(check_username("").is_err() && check_username("a b").is_err() && check_username("ä").is_err());
        assert!(check_password("1234567").is_err() && check_password("12345678").is_ok());
        let h = hash_password("correct horse").unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(verify_password("correct horse", Some(&h)));
        assert!(!verify_password("wrong horse", Some(&h)));
        assert!(!verify_password("correct horse", None));
    }

    #[test]
    fn accounts_and_sessions() {
        let db = db();
        let h = "$argon2id$v=19$m=8,t=1,p=1$c2FsdHNhbHQ$aGFzaA";
        let a = create_first(&db, "Alice", h).unwrap();
        assert_eq!(create_first(&db, "bob", h).unwrap_err().status.as_u16(), 409);
        assert_eq!(create(&db, "alice", h).unwrap_err().status.as_u16(), 409, "names ignore case");
        let b = create(&db, "bob", h).unwrap();
        assert_eq!(find(&db, "ALICE").unwrap().unwrap().0, a);
        assert_eq!(list(&db).unwrap().iter().map(|u| u.username.as_str()).collect::<Vec<_>>(), ["Alice", "bob"]);

        let t1 = create_session(&db, &a.id).unwrap();
        let t2 = create_session(&db, &a.id).unwrap();
        assert_eq!(session_user(&db, &t1).unwrap(), Some(a.clone()));
        assert_eq!(session_user(&db, "nope").unwrap(), None);
        set_password(&db, &a.id, h, Some(&t2)).unwrap();
        assert_eq!(session_user(&db, &t1).unwrap(), None, "a password change ends other sessions");
        assert!(session_user(&db, &t2).unwrap().is_some(), "but keeps the one that changed it");
        delete_session(&db, &t2).unwrap();
        assert_eq!(session_user(&db, &t2).unwrap(), None);

        let t3 = create_session(&db, &b.id).unwrap();
        delete(&db, &b.id).unwrap();
        assert_eq!(session_user(&db, &t3).unwrap(), None);
        assert_eq!(delete(&db, &a.id).unwrap_err().status.as_u16(), 409, "the last account stays");
    }
}
