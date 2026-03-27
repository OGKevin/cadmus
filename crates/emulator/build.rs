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

/// Ensures `raw` is a `GitVersion`-parseable string.
///
/// `git describe --tags --always --dirty` produces a tagged version like
/// `v0.10.0-5-gabc1234` when tags are reachable.  When no tag is reachable
/// (shallow clone, pre-first-release, or git unavailable) it falls back to a
/// bare commit hash such as `637b734` or `637b734-dirty`, which is not
/// parseable by `GitVersion::parse`.  This function wraps such raw strings
/// into the form `v0.0.0-0-g<hash>` so `GIT_VERSION` is always valid.
fn ensure_valid_version(raw: String) -> String {
    if raw.starts_with('v') {
        return raw;
    }
    let (hash, suffix) = if let Some(h) = raw.strip_suffix("-dirty") {
        (h, "-dirty")
    } else {
        (raw.as_str(), "")
    };
    let hash = if hash.is_empty() { "unknown" } else { hash };
    format!("v0.0.0-0-g{}{}", &hash[..hash.len().min(7)], suffix)
}

fn get_version_info() -> Result<(String, Option<String>), VarError> {
    let raw = Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .output()
        .ok()
        .and_then(|output| {
            output
                .status
                .success()
                .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        })
        .unwrap_or_else(|| "unknown".to_string());

    // When no tag is reachable (shallow clone, pre-first-release repo, or git
    // unavailable), `git describe --always` falls back to a bare commit hash
    // like `637b734` or `637b734-dirty`.  That string does not start with `v`
    // and fails the `GitVersion::parse` call in `get_current_version()`, which
    // panics under the `emulator` feature flag.  Wrap it into a parseable
    // development version so the build always produces a valid GIT_VERSION.
    let git_version = ensure_valid_version(raw);

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
