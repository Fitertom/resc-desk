//! The build's version for the updater (update.rs): `r<commit time>-<short commit>`.
use std::process::Command;

fn main() {
    let v = Command::new("git")
        .args(["log", "-1", "--format=%ct-%h"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .map(|s| format!("r{s}"))
        .unwrap_or_else(|| "dev".into());
    // A release build may name itself (scripts/release.sh).
    let v = std::env::var("RESC_BUILD_OVERRIDE").unwrap_or(v);
    println!("cargo:rustc-env=RESC_BUILD={v}");
    println!("cargo:rerun-if-env-changed=RESC_BUILD_OVERRIDE");
    for f in ["../../.git/HEAD", "../../.git/refs/heads", "../../.git/packed-refs"] {
        println!("cargo:rerun-if-changed={f}");
    }
}
