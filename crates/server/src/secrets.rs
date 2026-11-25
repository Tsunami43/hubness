//! Project secrets — a port of `secrets.ts`, same storage format, so either server reads
//! what the other wrote.
//!
//! Values are encrypted with AES-256-GCM, bound to (project, name) via AAD, and stored as
//! `v1:<key id>:<iv>:<tag>:<ciphertext>` (base64). Keys: `HUBNESS_SECRET_KEY` — one or more
//! comma-separated strings of ≥ 32 characters (SHA-256 is taken); the first encrypts, the
//! others only decrypt. Without it a key is generated in `<data>/secret.key` (mode 600).

use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use hubness_core::SecretInfo;
use rusqlite::Connection;
use sha2::{Digest, Sha256};

use crate::db::now;
use crate::error::{ApiResult, HttpError};

pub struct Key {
    pub id: String,
    key: [u8; 32],
}

fn random_bytes<const N: usize>() -> std::io::Result<[u8; N]> {
    let mut buf = [0u8; N];
    fs::File::open("/dev/urandom")?.read_exact(&mut buf)?;
    Ok(buf)
}

fn derive(material: &str) -> Key {
    let key: [u8; 32] = Sha256::digest(material.as_bytes()).into();
    let mut h = Sha256::new();
    h.update(b"hubness-secret-key:");
    h.update(key);
    let hex: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    Key { id: hex[..8].to_string(), key }
}

pub fn key_source() -> &'static str {
    if std::env::var("HUBNESS_SECRET_KEY").is_ok_and(|v| !v.is_empty()) { "env" } else { "file" }
}

/// The configured keys (the first one encrypts).
pub fn keys(data_dir: &Path) -> ApiResult<Vec<Key>> {
    let env = std::env::var("HUBNESS_SECRET_KEY").unwrap_or_default();
    let mut materials: Vec<String> = env.split(',').map(str::trim).filter(|k| !k.is_empty()).map(String::from).collect();
    if materials.iter().any(|m| m.chars().count() < 32) {
        return Err(HttpError::new(500, "Every HUBNESS_SECRET_KEY must be at least 32 characters"));
    }
    if materials.is_empty() {
        let file = data_dir.join("secret.key");
        if !file.exists() {
            fs::write(&file, format!("{}\n", B64.encode(random_bytes::<32>()?)))?;
        }
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600))?;
        materials.push(fs::read_to_string(&file)?.trim().to_string());
    }
    Ok(materials.iter().map(|m| derive(m)).collect())
}

/// Keys from explicit key materials (tests: vectors produced with a fixed HUBNESS_SECRET_KEY).
#[cfg(test)]
pub fn keys_from(materials: &[&str]) -> Vec<Key> {
    materials.iter().map(|m| derive(m)).collect()
}

fn aad(project_id: &str, name: &str) -> Vec<u8> {
    format!("{project_id}\0{name}").into_bytes()
}

pub fn encrypt(keys: &[Key], project_id: &str, name: &str, value: &str) -> ApiResult<String> {
    let k = keys.first().ok_or_else(|| HttpError::new(500, "No secret key"))?;
    let iv = random_bytes::<12>()?;
    let cipher = Aes256Gcm::new_from_slice(&k.key).map_err(|e| HttpError::new(500, e.to_string()))?;
    let aad = aad(project_id, name);
    let mut out = cipher
        .encrypt(&Nonce::try_from(&iv[..]).expect("12-byte nonce"), Payload { msg: value.as_bytes(), aad: &aad })
        .map_err(|_| HttpError::new(500, "Encryption failed"))?;
    let tag = out.split_off(out.len() - 16);
    Ok(["v1", &k.id, &B64.encode(iv), &B64.encode(tag), &B64.encode(out)].join(":"))
}

pub fn decrypt(keys: &[Key], project_id: &str, name: &str, blob: &str) -> Result<String, String> {
    let parts: Vec<&str> = blob.split(':').collect();
    let [v, kid, iv, tag, ct] = parts.as_slice() else { return Err("Unknown secret format".into()) };
    if *v != "v1" || kid.is_empty() || iv.is_empty() || tag.is_empty() {
        return Err("Unknown secret format".into());
    }
    let k = keys.iter().find(|k| k.id == *kid).ok_or_else(|| format!("Secret key {kid} is not configured"))?;
    let b = |s: &str| B64.decode(s).map_err(|e| e.to_string());
    let (iv, mut data) = (b(iv)?, b(ct)?);
    data.extend(b(tag)?);
    if iv.len() != 12 {
        return Err("Unknown secret format".into());
    }
    let cipher = Aes256Gcm::new_from_slice(&k.key).map_err(|e| e.to_string())?;
    let aad = aad(project_id, name);
    let nonce = Nonce::try_from(&iv[..]).map_err(|_| "Unknown secret format")?;
    let plain =
        cipher.decrypt(&nonce, Payload { msg: &data, aad: &aad }).map_err(|_| "Unsupported state or unable to authenticate data")?;
    String::from_utf8(plain).map_err(|e| e.to_string())
}

// ---------- project secrets ----------

struct Row {
    name: String,
    value: String,
    updated_at: String,
}

fn rows(db: &Connection, project_id: &str) -> rusqlite::Result<Vec<Row>> {
    let mut st = db.prepare("SELECT name, value, updated_at FROM secrets WHERE project_id = ? ORDER BY name")?;
    st.query_map([project_id], |r| Ok(Row { name: r.get(0)?, value: r.get(1)?, updated_at: r.get(2)? }))?.collect()
}

pub fn list(db: &Connection, project_id: &str) -> ApiResult<Vec<SecretInfo>> {
    Ok(rows(db, project_id)?.into_iter().map(|r| SecretInfo { name: r.name, updated_at: r.updated_at }).collect())
}

/// Variables that break startup or hijack the hub.
const RESERVED: &[&str] = &["PATH", "HOME", "USER", "SHELL", "TERM", "PWD", "LD_PRELOAD", "LD_LIBRARY_PATH", "NODE_OPTIONS"];

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && name.len() <= 128
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub fn set(db: &Connection, data_dir: &Path, project_id: &str, name: &str, value: &str) -> ApiResult<Vec<SecretInfo>> {
    if !valid_name(name) {
        return Err(HttpError::bad_request("Name: letters, digits and _ (like an env variable), not starting with a digit"));
    }
    if RESERVED.contains(&name.to_ascii_uppercase().as_str()) || name.starts_with("HUB_") || name.starts_with("HUBNESS_") {
        return Err(HttpError::bad_request(format!("{name} is reserved")));
    }
    if value.is_empty() {
        return Err(HttpError::bad_request("Value is required"));
    }
    if value.encode_utf16().count() > 64 * 1024 {
        return Err(HttpError::bad_request("Value is too long (max 64 KB)"));
    }
    let blob = encrypt(&keys(data_dir)?, project_id, name, value)?;
    db.execute(
        "INSERT INTO secrets (project_id, name, value, updated_at) VALUES (?, ?, ?, ?)
         ON CONFLICT(project_id, name) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        (project_id, name, blob, now()),
    )?;
    list(db, project_id)
}

pub fn delete(db: &Connection, project_id: &str, name: &str) -> ApiResult<Vec<SecretInfo>> {
    db.execute("DELETE FROM secrets WHERE project_id = ? AND name = ?", (project_id, name))?;
    list(db, project_id)
}

/// Secrets that can't be decrypted with the current keys (key rotated without the old one).
pub fn unreadable(db: &Connection, data_dir: &Path, project_id: &str) -> ApiResult<Vec<String>> {
    let keys = keys(data_dir)?;
    Ok(rows(db, project_id)?.into_iter().filter(|r| decrypt(&keys, project_id, &r.name, &r.value).is_err()).map(|r| r.name).collect())
}

/// Environment from project secrets — only for starting processes on the server.
/// Unreadable secrets are skipped (they never fail a run).
pub fn env(db: &Connection, data_dir: &Path, project_id: &str) -> ApiResult<Vec<(String, String)>> {
    let keys = keys(data_dir)?;
    Ok(rows(db, project_id)?.into_iter().filter_map(|r| decrypt(&keys, project_id, &r.name, &r.value).ok().map(|v| (r.name, v))).collect())
}

// ---------- routes ----------

use axum::Router;
use axum::extract::{Path as UrlPath, State};
use axum::routing::{get, put};

pub fn routes() -> Router<crate::AppState> {
    Router::new()
        .route("/projects/{id}/secrets", get(list_route))
        .route("/projects/{id}/secrets/{name}", put(set_route).delete(delete_route))
}

async fn list_route(State(s): State<crate::AppState>, UrlPath(id): UrlPath<String>) -> ApiResult<axum::Json<serde_json::Value>> {
    s.with(|db, cfg| {
        let p = crate::catalog::project_or_404(db, &id)?;
        Ok(serde_json::json!({ "secrets": list(db, &p.id)?, "unreadable": unreadable(db, &cfg.data_dir, &p.id)?, "keySource": key_source() }))
    })
}

#[derive(serde::Deserialize)]
struct SecretBody {
    value: String,
}

async fn set_route(
    State(s): State<crate::AppState>,
    UrlPath((id, name)): UrlPath<(String, String)>,
    crate::http::JsonBody(b): crate::http::JsonBody<SecretBody>,
) -> ApiResult<axum::Json<Vec<SecretInfo>>> {
    if b.value.encode_utf16().count() > 64_000 {
        return Err(HttpError::bad_request("value: at most 64000 characters"));
    }
    s.with(|db, cfg| {
        let p = crate::catalog::project_or_404(db, &id)?;
        set(db, &cfg.data_dir, &p.id, &name, &b.value)
    })
}

async fn delete_route(
    State(s): State<crate::AppState>,
    UrlPath((id, name)): UrlPath<(String, String)>,
) -> ApiResult<axum::Json<Vec<SecretInfo>>> {
    s.with(|db, _| {
        let p = crate::catalog::project_or_404(db, &id)?;
        delete(db, &p.id, &name)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MATERIAL: &str = "test-key-material-that-is-32-chars-long!";

    #[test]
    fn reads_a_value_encrypted_by_the_typescript_server() {
        // produced by apps/server/src/secrets.ts encrypt('proj-1', 'API_TOKEN', …) with MATERIAL
        let blob = "v1:08cd82e9:WE3HWzoHNpv5g9Nj:4rzDRYvb7CLtcSR3fX/JUA==:t6RSBac6awW4j5Bu5OHZOzo=";
        let keys = vec![derive(MATERIAL)];
        assert_eq!(keys[0].id, "08cd82e9");
        assert_eq!(decrypt(&keys, "proj-1", "API_TOKEN", blob).unwrap(), "sécret ✓ value");
        assert!(decrypt(&keys, "proj-2", "API_TOKEN", blob).is_err(), "bound to the project");
        assert!(decrypt(&keys, "proj-1", "OTHER", blob).is_err(), "bound to the name");
    }

    #[test]
    fn round_trip_and_rotation() {
        let old = derive(MATERIAL);
        let blob = encrypt(std::slice::from_ref(&old), "p", "N", "v").unwrap();
        let rotated = vec![derive("another-key-material-32-characters-xx"), old];
        assert_eq!(decrypt(&rotated, "p", "N", &blob).unwrap(), "v", "old keys still decrypt");
        assert!(decrypt(&rotated[..1], "p", "N", &blob).unwrap_err().contains("not configured"));
    }

    #[test]
    fn names_like_env_variables() {
        assert!(valid_name("API_TOKEN") && valid_name("_x1"));
        assert!(!valid_name("1A") && !valid_name("A-B") && !valid_name(""));
    }
}
