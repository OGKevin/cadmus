//! Collect `git describe` versions for bundled C/C++ submodules.

use std::path::{Path, PathBuf};

use gix::ObjectId;
use gix::commit::describe::SelectRef;

use crate::versions::LIBRARY_NAMES;

/// Collects bundled third-party submodule versions for a Cadmus build.
///
/// Walks [`LIBRARY_NAMES`] in order and returns one `(name, version)` pair
/// per submodule. Versions match `git describe --tags --always --dirty`
/// of that submodule, with fallbacks to a module-git-dir describe, a short
/// gitlink SHA, then `"unknown"`.
pub fn collect_thirdparty_versions(root: &Path) -> Vec<(String, String)> {
    LIBRARY_NAMES
        .iter()
        .map(|name| ((*name).to_string(), version_for_submodule(root, name)))
        .collect()
}

/// Resolves the version stamp for one [`LIBRARY_NAMES`] submodule.
///
/// Prefers a working-tree describe of `thirdparty/<name>`, then a
/// module-git-dir describe of the gitlink commit, then the short gitlink
/// SHA. Returns `"unknown"` when none of those are available.
fn version_for_submodule(root: &Path, name: &str) -> String {
    describe_working_tree(root, name)
        .or_else(|| describe_module_git_dir(root, name))
        .or_else(|| short_gitlink_sha(root, name))
        .unwrap_or_else(|| "unknown".to_string())
}

fn describe_working_tree(root: &Path, name: &str) -> Option<String> {
    let repo = open_repo(&root.join("thirdparty").join(name))?;
    let commit = repo.head_commit().ok()?;
    describe_commit(&commit, true)
}

fn describe_module_git_dir(root: &Path, name: &str) -> Option<String> {
    let git_dir = submodule_git_dir(root, name)?;
    let oid = gitlink_id(root, name)?;
    let repo = open_repo(&git_dir)?;
    let commit = repo.find_object(oid).ok()?.try_into_commit().ok()?;
    describe_commit(&commit, false)
}

/// Git directory of the `thirdparty/<name>` submodule, if present.
///
/// Uses the repository common dir so Git worktrees (where `.git` is a
/// file) still resolve `$GIT_COMMON_DIR/modules/thirdparty/<name>`.
pub fn submodule_git_dir(root: &Path, name: &str) -> Option<PathBuf> {
    let git_dir = open_repo(root)?
        .common_dir()
        .join("modules/thirdparty")
        .join(name);
    git_dir.exists().then_some(git_dir)
}

fn gitlink_id(root: &Path, name: &str) -> Option<ObjectId> {
    let repo = open_repo(root)?;
    let spec = format!("HEAD:thirdparty/{name}");
    repo.rev_parse_single(spec.as_str())
        .ok()
        .map(|id| id.detach())
}

fn short_gitlink_sha(root: &Path, name: &str) -> Option<String> {
    let hex = gitlink_id(root, name)?.to_string();
    Some(hex.get(..7).unwrap_or(&hex).to_string())
}

fn describe_commit(commit: &gix::Commit<'_>, dirty: bool) -> Option<String> {
    let resolution = commit
        .describe()
        .names(SelectRef::AllTags)
        .id_as_fallback(true)
        .try_resolve()
        .ok()??;
    let format = if dirty {
        resolution
            .format_with_dirty_suffix(Some("dirty".into()))
            .ok()?
    } else {
        resolution.format().ok()?
    };
    Some(format.to_string()).filter(|version| !version.is_empty())
}

fn open_repo(path: &Path) -> Option<gix::Repository> {
    gix::open_opts(path, gix::open::Options::isolated()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gix::bstr::ByteSlice;
    use gix::objs::tree::EntryKind;
    use gix::refs::transaction::PreviousValue;
    use std::path::Path;

    fn test_signature() -> gix::actor::SignatureRef<'static> {
        gix::actor::SignatureRef {
            name: b"test".as_bstr(),
            email: b"test@example.com".as_bstr(),
            time: "1 +0000",
        }
    }

    fn open_test_repo(dir: &Path) -> gix::Repository {
        gix::open_opts(dir, gix::open::Options::isolated()).expect("open test repo")
    }

    fn commit_tree(repo: &gix::Repository, tree: gix::ObjectId, message: &str) -> gix::ObjectId {
        let signature = test_signature();
        repo.commit_as(
            signature,
            signature,
            "HEAD",
            message,
            tree,
            None::<gix::ObjectId>,
        )
        .expect("commit")
        .detach()
    }

    fn write_tree(
        repo: &gix::Repository,
        path: &str,
        kind: EntryKind,
        id: gix::ObjectId,
    ) -> gix::ObjectId {
        let mut editor = repo.empty_tree().edit().expect("edit empty tree");
        editor.upsert(path, kind, id).expect("upsert tree entry");
        editor.write().expect("write tree").detach()
    }

    fn write_worktree_index(repo: &gix::Repository, tree: gix::ObjectId) {
        let mut index = repo.index_from_tree(&tree).expect("index from tree");
        index.write(Default::default()).expect("write index");
    }

    fn init_repo(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        gix::init(dir).expect("init repo");
    }

    fn commit_and_tag(dir: &Path, tag: &str) {
        std::fs::write(dir.join("README"), "test\n").unwrap();
        let repo = open_test_repo(dir);
        let blob = repo.write_blob(b"test\n").expect("write blob").detach();
        let tree = write_tree(&repo, "README", EntryKind::Blob, blob);
        let commit = commit_tree(&repo, tree, "init");
        repo.tag_reference(tag, commit, PreviousValue::MustNotExist)
            .expect("create tag");
        write_worktree_index(&repo, tree);
    }

    fn commit_gitlink(dir: &Path, gitlink: gix::ObjectId, message: &str) {
        let repo = open_test_repo(dir);
        let tree = write_tree(&repo, "thirdparty/libwebp", EntryKind::Commit, gitlink);
        commit_tree(&repo, tree, message);
    }

    fn libwebp_version(root: &Path) -> Option<String> {
        collect_thirdparty_versions(root)
            .into_iter()
            .find(|(name, _)| name == "libwebp")
            .map(|(_, version)| version)
    }

    #[test]
    fn collect_thirdparty_versions_reports_unknown_when_submodule_dirs_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let versions = collect_thirdparty_versions(tmp.path());
        assert_eq!(
            versions
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            LIBRARY_NAMES.to_vec()
        );
        assert!(
            versions.iter().all(|(_, version)| version == "unknown"),
            "missing submodules should report unknown: {versions:?}"
        );
    }

    #[test]
    fn collect_thirdparty_versions_uses_git_describe_from_working_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let libwebp = tmp.path().join("thirdparty/libwebp");
        init_repo(&libwebp);
        commit_and_tag(&libwebp, "v1.6.0");

        assert_eq!(libwebp_version(tmp.path()).as_deref(), Some("v1.6.0"));

        let mupdf_version = collect_thirdparty_versions(tmp.path())
            .into_iter()
            .find(|(name, _)| name == "mupdf")
            .map(|(_, version)| version);
        assert_eq!(mupdf_version.as_deref(), Some("unknown"));
    }

    #[test]
    fn collect_thirdparty_versions_appends_dirty_suffix() {
        let tmp = tempfile::tempdir().unwrap();
        let libwebp = tmp.path().join("thirdparty/libwebp");
        init_repo(&libwebp);
        commit_and_tag(&libwebp, "v1.6.0");
        std::fs::write(libwebp.join("README"), "dirty\n").unwrap();

        assert_eq!(libwebp_version(tmp.path()).as_deref(), Some("v1.6.0-dirty"));
    }

    #[test]
    fn collect_thirdparty_versions_ignores_parent_repo_describe() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);
        commit_and_tag(root, "v9.9.9");
        std::fs::create_dir_all(root.join("thirdparty/libwebp")).unwrap();

        assert_eq!(libwebp_version(root).as_deref(), Some("unknown"));
    }

    #[test]
    fn collect_thirdparty_versions_uses_module_git_dir_when_tree_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);

        let module_git_dir = root.join(".git/modules/thirdparty/libwebp");
        std::fs::create_dir_all(module_git_dir.parent().unwrap()).unwrap();
        let module = gix::init_bare(&module_git_dir).expect("init bare module git dir");
        let blob = module.write_blob(b"test\n").expect("write blob").detach();
        let tree = write_tree(&module, "README", EntryKind::Blob, blob);
        let gitlink = commit_tree(&module, tree, "init");
        module
            .tag_reference("v1.6.0", gitlink, PreviousValue::MustNotExist)
            .expect("create tag");

        commit_gitlink(root, gitlink, "gitlink");
        std::fs::create_dir_all(root.join("thirdparty/libwebp")).unwrap();

        assert_eq!(libwebp_version(root).as_deref(), Some("v1.6.0"));
    }

    #[test]
    fn collect_thirdparty_versions_falls_back_to_short_gitlink_sha() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);
        let gitlink = gix::ObjectId::from_hex(b"0123456789abcdef0123456789abcdef01234567")
            .expect("gitlink oid");
        commit_gitlink(root, gitlink, "gitlink");

        assert_eq!(libwebp_version(root).as_deref(), Some("0123456"));
    }
}
