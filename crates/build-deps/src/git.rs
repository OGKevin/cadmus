//! Repository access shared by the build helpers.

use std::path::Path;

/// Opens the git repository at `path` with isolated options, ignoring the
/// caller's user and system git configuration.
///
/// Returns `None` when `path` is not inside a repository, so tarball builds
/// degrade to their documented fallbacks instead of failing.
pub fn open_repo(path: &Path) -> Option<gix::Repository> {
    gix::open_opts(path, gix::open::Options::isolated()).ok()
}
