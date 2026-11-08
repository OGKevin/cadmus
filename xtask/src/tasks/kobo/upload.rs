//! SFTP upload of the assembled distribution to the device.
//!
//! Deploy only adds or overwrites files: a reader's own fonts, dictionaries,
//! and stylesheets stay put even when the distribution stops shipping a file
//! that shares their directory. `/mnt/onboard` is FAT, so mode and timestamps
//! are never synced.
//!
//! Unchanged files are skipped using a manifest of what the last upload wrote:
//! the host hashes `dist/` locally, and one SFTP stat per remote file confirms
//! it still has the recorded size. Nothing is hashed on the device, so the
//! Kobo pays no CPU for the comparison.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use bytesize::ByteSize;
use russh::client::Handle;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use walkdir::WalkDir;

use super::INSTALL_DIR;
use super::ssh::{self, Handler};

/// One path to create on the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UploadEntry {
    /// Remote directory to create when missing.
    Dir { remote: String },
    /// Local file to send to `remote`.
    File { local: PathBuf, remote: String },
}

impl UploadEntry {
    pub fn remote(&self) -> &str {
        match self {
            Self::Dir { remote } | Self::File { remote, .. } => remote,
        }
    }
}

/// Maps `dist/` onto ordered upload entries: directories first, then files.
///
/// Sorting by remote path keeps every parent ahead of its children, so a file
/// is never written before the directory that holds it exists.
pub(crate) fn plan(dist: &Path) -> Result<Vec<UploadEntry>> {
    let mut dirs = Vec::new();
    let mut files = Vec::new();

    for entry in WalkDir::new(dist).min_depth(1).follow_links(false) {
        let entry = entry.context("failed to walk dist/")?;
        let relative = entry.path().strip_prefix(dist).with_context(|| {
            format!("{} is not under {}", entry.path().display(), dist.display())
        })?;
        let remote = format!(
            "{INSTALL_DIR}/{}",
            relative.to_string_lossy().replace('\\', "/")
        );

        if entry.file_type().is_symlink() {
            println!(
                "Skipping symlink {} (deploy uploads regular files only)",
                entry.path().display()
            );
            continue;
        }

        if entry.file_type().is_dir() {
            dirs.push(UploadEntry::Dir { remote });
        } else {
            files.push(UploadEntry::File {
                local: entry.path().to_path_buf(),
                remote,
            });
        }
    }

    dirs.sort_by(|left, right| left.remote().cmp(right.remote()));
    files.sort_by(|left, right| left.remote().cmp(right.remote()));
    dirs.extend(files);
    Ok(dirs)
}

/// What the last deploy uploaded, keyed `host:remote path`.
///
/// The hash is computed on the host, so skipping costs one SFTP stat per
/// remote file. A same-size edit made on the device outside deploy is not
/// detected, which matches rsync's default quick check and avoids hashing the
/// whole tree on the Kobo.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    entries: BTreeMap<String, ManifestEntry>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ManifestEntry {
    size: u64,
    hash: String,
}

/// Where the upload manifest lives.
///
/// The manifest remembers what the last deploy wrote to each device so the
/// next one can skip unchanged files after a single SFTP stat per path.
pub(crate) fn manifest_path(root: &Path) -> PathBuf {
    root.join("target/kobo-deploy-manifest.json")
}

fn manifest_key(host: &str, remote: &str) -> String {
    format!("{host}:{remote}")
}

/// Whether the local file differs from what was uploaded to `remote`.
fn should_upload(
    entry: Option<&ManifestEntry>,
    local_size: u64,
    local_hash: &str,
    remote_size: Option<u64>,
) -> bool {
    match (entry, remote_size) {
        (Some(entry), Some(remote_size)) => {
            entry.size != local_size || entry.hash != local_hash || remote_size != local_size
        }
        _ => true,
    }
}

/// Hashes every local file with blake3, keyed by remote path.
///
/// Reads are async; only the hashing itself runs on the blocking pool, since
/// it is CPU work with no async equivalent.
async fn compute_digests(entries: &[UploadEntry]) -> Result<BTreeMap<String, (u64, String)>> {
    let mut digests = BTreeMap::new();

    for entry in entries {
        let UploadEntry::File { local, remote } = entry else {
            continue;
        };
        let bytes = tokio::fs::read(local)
            .await
            .with_context(|| format!("failed to read {}", local.display()))?;
        let size = bytes.len() as u64;
        let hash = tokio::task::spawn_blocking(move || blake3::hash(&bytes).to_hex().to_string())
            .await
            .context("failed to hash a local file")?;
        digests.insert(remote.clone(), (size, hash));
    }

    Ok(digests)
}

fn load_manifest(path: &Path) -> Manifest {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_manifest(path: &Path, manifest: &Manifest) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(manifest)
        .context("failed to serialize the upload manifest")?;
    std::fs::write(path, text).with_context(|| format!("failed to write {}", path.display()))
}

/// Uploads `dist/` into [`INSTALL_DIR`], creating directories as needed.
pub(crate) async fn run(
    handle: &mut Handle<Handler>,
    host: &str,
    dist: &Path,
    manifest_path: &Path,
) -> Result<()> {
    let mkdir = ssh::exec(handle, &format!("mkdir -p {INSTALL_DIR}")).await?;
    if mkdir.exit_status != Some(0) {
        bail!(
            "mkdir -p {INSTALL_DIR} failed on the device:\n{}{}",
            mkdir.stdout,
            mkdir.stderr
        );
    }

    let channel = handle.channel_open_session().await?;
    channel.request_subsystem(true, "sftp").await?;
    let sftp = SftpSession::new(channel.into_stream())
        .await
        .context("failed to start the SFTP subsystem")?;

    let dist = dist.to_path_buf();
    let entries = crate::blocking(move || plan(&dist)).await?;
    let local_digests = compute_digests(&entries).await?;
    let manifest_path = manifest_path.to_path_buf();
    let mut manifest = {
        let path = manifest_path.clone();
        tokio::task::spawn_blocking(move || load_manifest(&path)).await?
    };
    let mut uploaded_files = 0usize;
    let mut uploaded_bytes = 0u64;
    let mut skipped_files = 0usize;
    let mut skipped_bytes = 0u64;

    for entry in &entries {
        match entry {
            UploadEntry::Dir { remote } => {
                if !sftp
                    .try_exists(remote)
                    .await
                    .with_context(|| format!("failed to stat {remote}"))?
                {
                    sftp.create_dir(remote)
                        .await
                        .with_context(|| format!("failed to create {remote}"))?;
                }
            }
            UploadEntry::File { local, remote } => {
                let (local_size, local_hash) = local_digests
                    .get(remote)
                    .with_context(|| format!("missing digest for {remote}"))?;
                let remote_size = sftp
                    .metadata(remote)
                    .await
                    .ok()
                    .and_then(|metadata| metadata.size);
                let key = manifest_key(host, remote);

                if !should_upload(
                    manifest.entries.get(&key),
                    *local_size,
                    local_hash,
                    remote_size,
                ) {
                    skipped_files += 1;
                    skipped_bytes += local_size;
                    continue;
                }

                uploaded_bytes += upload_file(&sftp, local, remote).await?;
                uploaded_files += 1;
                manifest.entries.insert(
                    key,
                    ManifestEntry {
                        size: *local_size,
                        hash: local_hash.clone(),
                    },
                );
            }
        }
    }

    sftp.close()
        .await
        .context("failed to close the SFTP session")?;

    let path = manifest_path.clone();
    let saved = manifest;
    crate::blocking(move || save_manifest(&path, &saved)).await?;

    println!(
        "Uploaded {uploaded_files} files ({}) to {INSTALL_DIR}",
        ByteSize(uploaded_bytes)
    );
    if skipped_files > 0 {
        println!(
            "Skipped {skipped_files} unchanged files ({})",
            ByteSize(skipped_bytes)
        );
    }
    Ok(())
}

/// Uploads one file, staging it beside the target and renaming it into place.
///
/// The running Cadmus binary cannot be opened for writing (`ETXTBSY`), and a
/// mapped shared library should not be truncated in place, so the bytes go to
/// [`staging_path`] first. The rename replaces the target; servers that refuse
/// renaming over an existing path fall back to removing it first.
async fn upload_file(sftp: &SftpSession, local: &Path, remote: &str) -> Result<u64> {
    let staging = staging_path(remote);
    let mut source = tokio::fs::File::open(local)
        .await
        .with_context(|| format!("failed to open {}", local.display()))?;
    let mut target = sftp
        .open_with_flags(
            &staging,
            OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE,
        )
        .await
        .with_context(|| format!("failed to open {staging}"))?;
    let bytes = tokio::io::copy(&mut source, &mut target)
        .await
        .with_context(|| format!("failed to upload {staging}"))?;
    target
        .shutdown()
        .await
        .with_context(|| format!("failed to close {staging}"))?;

    replace_file(sftp, &staging, remote).await?;
    Ok(bytes)
}

/// Renames `staging` over `remote`, removing the target first when the server
/// refuses to rename onto an existing path.
async fn replace_file(sftp: &SftpSession, staging: &str, remote: &str) -> Result<()> {
    if sftp.rename(staging, remote).await.is_ok() {
        return Ok(());
    }

    match sftp.remove_file(remote).await {
        Ok(()) => {}
        Err(russh_sftp::client::error::Error::Status(status))
            if status.status_code == russh_sftp::protocol::StatusCode::NoSuchFile => {}
        Err(error) => {
            bail!("failed to replace {remote}: removing the old file failed with {error}");
        }
    }

    sftp.rename(staging, remote)
        .await
        .with_context(|| format!("failed to replace {remote}"))
}

/// Staging name used beside the target so a rename can replace it.
fn staging_path(remote: &str) -> String {
    format!("{remote}.cadmus-tmp")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_paths_sit_beside_the_target() {
        assert_eq!(
            staging_path(&format!("{INSTALL_DIR}/cadmus")),
            format!("{INSTALL_DIR}/cadmus.cadmus-tmp")
        );
    }

    #[test]
    fn upload_skips_only_identical_state() {
        let entry = ManifestEntry {
            size: 42,
            hash: "abc".to_string(),
        };

        assert!(!should_upload(Some(&entry), 42, "abc", Some(42)));
        assert!(should_upload(None, 42, "abc", Some(42)));
        assert!(should_upload(Some(&entry), 43, "abc", Some(43)));
        assert!(should_upload(Some(&entry), 42, "def", Some(42)));
        assert!(should_upload(Some(&entry), 42, "abc", Some(41)));
        assert!(should_upload(Some(&entry), 42, "abc", None));
    }

    #[test]
    fn manifest_round_trips_through_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.json");
        let mut manifest = Manifest::default();
        manifest.entries.insert(
            manifest_key("kobo", &format!("{INSTALL_DIR}/cadmus")),
            ManifestEntry {
                size: 42,
                hash: "abc".to_string(),
            },
        );

        save_manifest(&path, &manifest).unwrap();
        let loaded = load_manifest(&path);

        assert_eq!(loaded.entries, manifest.entries);
    }

    #[tokio::test]
    async fn digests_cover_every_planned_file() {
        let dir = tempfile::tempdir().unwrap();
        let dist = dir.path().join("dist");
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::write(dist.join("cadmus"), b"binary").unwrap();

        let entries = plan(&dist).unwrap();
        let digests = compute_digests(&entries).await.unwrap();

        assert_eq!(digests.len(), 1);
        assert_eq!(
            digests[&format!("{INSTALL_DIR}/cadmus")].0,
            b"binary".len() as u64
        );
    }

    #[test]
    fn plan_puts_directories_first_in_path_order() {
        let dir = tempfile::tempdir().unwrap();
        let dist = dir.path().join("dist");
        std::fs::create_dir_all(dist.join("libs")).unwrap();
        std::fs::create_dir_all(dist.join("fonts")).unwrap();
        std::fs::write(dist.join("cadmus"), b"binary").unwrap();
        std::fs::write(dist.join("libs/libfoo.so"), b"library").unwrap();
        std::fs::write(dist.join("fonts/font.css"), b"style").unwrap();

        let entries = plan(&dist).unwrap();

        assert_eq!(
            entries,
            vec![
                UploadEntry::Dir {
                    remote: format!("{INSTALL_DIR}/fonts"),
                },
                UploadEntry::Dir {
                    remote: format!("{INSTALL_DIR}/libs"),
                },
                UploadEntry::File {
                    local: dist.join("cadmus"),
                    remote: format!("{INSTALL_DIR}/cadmus"),
                },
                UploadEntry::File {
                    local: dist.join("fonts/font.css"),
                    remote: format!("{INSTALL_DIR}/fonts/font.css"),
                },
                UploadEntry::File {
                    local: dist.join("libs/libfoo.so"),
                    remote: format!("{INSTALL_DIR}/libs/libfoo.so"),
                },
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn plan_skips_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let dist = dir.path().join("dist");
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::write(dist.join("cadmus"), b"binary").unwrap();
        std::os::unix::fs::symlink("cadmus", dist.join("cadmus-link")).unwrap();

        let entries = plan(&dist).unwrap();

        assert_eq!(
            entries,
            vec![UploadEntry::File {
                local: dist.join("cadmus"),
                remote: format!("{INSTALL_DIR}/cadmus"),
            }]
        );
    }
}
