//! Bakes the target triple and the build flavor into the binary: `hubness update` downloads
//! the release archive for exactly this target and flavor.

fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    let flavor = if std::env::var_os("CARGO_FEATURE_SERVER").is_some() { "full" } else { "client" };
    println!("cargo:rustc-env=HUBNESS_TARGET={target}");
    println!("cargo:rustc-env=HUBNESS_FLAVOR={flavor}");
    println!("cargo:rerun-if-changed=build.rs");
}
