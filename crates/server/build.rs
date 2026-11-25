//! With the `embed-web` feature: generate a table of every file of the built web app
//! (`apps/web/dist`, or `HUBNESS_EMBED_WEB_DIST`) for `include_bytes!`, served by `web.rs`.
//! Without the feature nothing is generated and no web build is needed.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=HUBNESS_EMBED_WEB_DIST");
    if std::env::var_os("CARGO_FEATURE_EMBED_WEB").is_none() {
        return;
    }
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let dist = match std::env::var_os("HUBNESS_EMBED_WEB_DIST") {
        Some(d) => PathBuf::from(d),
        None => manifest.join("../../apps/web/dist"),
    };
    let dist = dist.canonicalize().unwrap_or(dist);
    // the whole folder: cargo rebuilds when any file in it changes
    println!("cargo:rerun-if-changed={}", dist.display());
    if !dist.join("index.html").is_file() {
        panic!(
            "\n\nThe `embed-web` feature compiles the web app into the binary, but {} has no index.html.\n\
             Build the web app first:  pnpm install && pnpm build   (without pnpm: npx -y pnpm@11.29.0 install && npx -y pnpm@11.29.0 --filter @hubness/web run build)\n\
             or point HUBNESS_EMBED_WEB_DIST at a built web app, or build without `--features embed-web`.\n",
            dist.display()
        );
    }
    let mut files = Vec::new();
    collect(&dist, &dist, &mut files);
    files.sort();
    let mut out = String::from("pub static ASSETS: &[Asset] = &[\n");
    for (name, path) in &files {
        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        writeln!(
            out,
            "    Asset {{ path: {name:?}, etag: \"\\\"{:016x}\\\"\", bytes: include_bytes!({:?}) }},",
            fnv1a(&bytes),
            path.display().to_string()
        )
        .unwrap();
    }
    out.push_str("];\n");
    let target = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("web_assets.rs");
    std::fs::write(target, out).expect("write web_assets.rs");
}

/// Files under `dir` as (path relative to `root` with `/` separators, absolute path).
fn collect(root: &Path, dir: &Path, files: &mut Vec<(String, PathBuf)>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect(root, &path, files);
        } else if path.is_file() {
            let rel = path.strip_prefix(root).expect("inside dist");
            let name = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
            files.push((name, path));
        }
    }
}

/// A content hash for the ETag (64-bit FNV-1a: no dependency needed).
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3))
}
