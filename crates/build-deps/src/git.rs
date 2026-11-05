//! Repository access and the git-describe version string shared by the build
//! helpers.

use std::path::Path;

/// Opens the git repository at `path` with isolated options, ignoring the
/// caller's user and system git configuration.
///
/// Returns `None` when `path` is not inside a repository, so tarball builds
/// degrade to their documented fallbacks instead of failing.
pub fn open_repo(path: &Path) -> Option<gix::Repository> {
    gix::open_opts(path, gix::open::Options::isolated()).ok()
}

/// `vX.Y.Z`, then `-N-ghash` when the commit is ahead of the tag, then
/// `-dirty`.
///
/// `root` is the repository root. A repository that cannot be read, or a
/// `HEAD` without a `vX.Y.Z` tag, yields `v0.0.0`.
pub fn git_version(root: &Path) -> String {
    let Some(repo) = open_repo(root) else {
        return "v0.0.0".to_string();
    };
    let Ok(head) = repo.head_commit() else {
        return "v0.0.0".to_string();
    };
    let Ok(Some(resolution)) = head
        .describe()
        .names(gix::commit::describe::SelectRef::AllTags)
        .try_resolve()
    else {
        return "v0.0.0".to_string();
    };
    let Some(name) = resolution.outcome.name.as_deref() else {
        return "v0.0.0".to_string();
    };
    let Some((major, minor, patch)) = parse_tag(&String::from_utf8_lossy(name)) else {
        return "v0.0.0".to_string();
    };

    let mut version = format!("v{major}.{minor}.{patch}");
    let depth = resolution.outcome.depth;
    if depth > 0 {
        let short = resolution
            .id
            .shorten()
            .map(|id| id.to_string())
            .unwrap_or_else(|_| resolution.outcome.id.to_string());
        version.push_str(&format!("-{depth}-g{short}"));
    }
    if repo.is_dirty().unwrap_or(false) {
        version.push_str("-dirty");
    }
    version
}

/// Split `vX.Y.Z` into its three numbers. Extra components are rejected.
fn parse_tag(name: &str) -> Option<(u64, u64, u64)> {
    let rest = name.strip_prefix('v')?;
    let mut parts = rest.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use super::{git_version, parse_tag};
    use std::path::Path;

    #[test]
    fn parse_tag_accepts_exactly_three_numbers() {
        assert_eq!(parse_tag("v0.12.0"), Some((0, 12, 0)));
        assert_eq!(parse_tag("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_tag("0.12.0"), None);
        assert_eq!(parse_tag("v0.12"), None);
        assert_eq!(parse_tag("v0.12.0.1"), None);
        assert_eq!(parse_tag("v0.12.0-rc1"), None);
    }

    #[test]
    fn describes_the_workspace_checkout() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let version = git_version(&root);
        assert!(version.starts_with("v0."), "{version}");
        assert!(version.split('.').count() >= 3, "{version}");
    }

    #[test]
    fn missing_repository_yields_zero_version() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(git_version(dir.path()), "v0.0.0");
    }
}
