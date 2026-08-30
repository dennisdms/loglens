//! Build script. Stamps the commit the binary was built from into the
//! executable, so `--version` can report it.

fn main() {
    // Version string: 0.1.0 (a1b2c3d) — the SHA is what makes a bug report
    // against "0.1.0" actionable when several builds share that version.
    let sha = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        // "unknown" is the honest answer when building from a source archive
        // with no `.git` present. Never fail the build over it.
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=LOGLENS_GIT_SHA={sha}");
    println!("cargo:rerun-if-changed=.git/HEAD");
}
