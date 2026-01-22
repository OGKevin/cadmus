use std::env::{self, VarError};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    let (git_version, pr_info) = get_version_info().expect("Failed to get version info");
    println!("cargo:rustc-env=GIT_VERSION={}", git_version);
    if let Some(pr) = pr_info {
        println!("cargo:rustc-env=PR_INFO={}", pr);
    }
}

fn get_version_info() -> Result<(String, Option<String>), VarError> {
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
            output
                .status
                .success()
                .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        })
        .unwrap_or_else(|| "unknown".to_string());

    let ci_var = env::var("CI").ok();
    match ci_var {
        Some(_) => {
            if !env::var("GITHUB_EVENT_NAME")
                .unwrap_or_default()
                .starts_with("pull_request")
            {
                return Ok((git_version, None));
            }

            let pr_number = env::var("PR_NUMBER").expect("PR_NUMBER not set in CI environment");
            let mut pr_head_sha =
                env::var("PR_HEAD_SHA").expect("PR_HEAD_SHA not set in CI environment");
            pr_head_sha = pr_head_sha.get(..7).unwrap_or(&pr_head_sha).to_string();

            Ok((
                git_version,
                Some(format!("PR #{} ({})", pr_number, pr_head_sha)),
            ))
        }
        _ => Ok((git_version, None)),
    }
}
