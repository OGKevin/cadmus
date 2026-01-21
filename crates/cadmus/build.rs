use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");

    let git_version = Command::new("git")
        .args([
            "describe",
            "--tags",
            "--always",
            "--dirty",
            "--first-parent",
        ])
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=GIT_VERSION={}", git_version);
}
