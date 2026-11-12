//! Device filesystem RPCs shared by public and private listeners.

use super::handler::PublicRpc;
use crate::document::file_extension::{FileExtension, UnknownFileExtension};
use cadmus_api_proto::cadmus::api::shared::v1::{
    DirectoryEntry, DirectoryListing, Extension, FileContents, FileOps, GetFileRequest,
    ListDirectoryRequest,
};
use connectrpc::{ConnectError, Encodable, RequestContext, ServiceRequest, ServiceResult};
use std::future::Future;
use std::path::{Path, PathBuf};

include!(concat!(env!("OUT_DIR"), "/api_extension_impls.rs"));

/// Largest payload `GetFile` returns: 32 MiB (33,554,432 bytes), the same
/// limit the static site uses.
const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;

impl FileOps for PublicRpc {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    fn list_directory<'a>(
        &'a self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, ListDirectoryRequest>,
    ) -> impl Future<Output = ServiceResult<impl Encodable<DirectoryListing> + Send + use<'a>>> + Send
    {
        let owned = request.to_owned_message();
        async move {
            let extensions: Vec<FileExtension> = owned
                .extensions
                .iter()
                .filter_map(|value| FileExtension::try_from(value.as_known()?).ok())
                .collect();
            list_device_directory(
                self.file_root.as_deref().map(PathBuf::as_path),
                &owned.path,
                &extensions,
            )
            .await
            .and_then(connectrpc::Response::ok)
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    fn get_file<'a>(
        &'a self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, GetFileRequest>,
    ) -> impl Future<Output = ServiceResult<impl Encodable<FileContents> + Send + use<'a>>> + Send
    {
        let owned = request.to_owned_message();
        async move {
            get_device_file(self.file_root.as_deref().map(PathBuf::as_path), &owned.path)
                .await
                .and_then(connectrpc::Response::ok)
        }
    }
}

/// Lists immediate children of `path`.
///
/// An empty `path` starts at the process current directory. An empty
/// `extensions` list returns directories only. Otherwise directories and files
/// whose suffix matches one of `extensions` are included.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(path, extensions = extensions.len())))]
async fn list_device_directory(
    root: Option<&Path>,
    path: &str,
    extensions: &[FileExtension],
) -> Result<DirectoryListing, ConnectError> {
    let requested = if path.trim().is_empty() {
        PathBuf::from(".")
    } else {
        PathBuf::from(path)
    };
    let canonical = tokio::fs::canonicalize(&requested)
        .await
        .map_err(|_| ConnectError::invalid_argument("invalid path"))?;
    confine(root, &canonical).await?;
    let metadata = tokio::fs::metadata(&canonical)
        .await
        .map_err(|_| ConnectError::invalid_argument("invalid path"))?;
    if !metadata.is_dir() {
        return Err(ConnectError::invalid_argument("path is not a directory"));
    }
    let mut entries = Vec::new();
    let mut reader = tokio::fs::read_dir(&canonical)
        .await
        .map_err(|_| ConnectError::invalid_argument("invalid path"))?;
    while let Some(entry) = crate::fs::next_dir_entry(&mut reader).await {
        let Ok(metadata) = entry.metadata().await else {
            continue;
        };
        let directory = metadata.is_dir();
        if !directory
            && !(metadata.is_file() && extension_allowed(entry.path().as_path(), extensions))
        {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        entries.push(DirectoryEntry {
            name,
            path: entry.path().to_string_lossy().into_owned(),
            directory,
            ..Default::default()
        });
    }
    entries.sort_by_key(|entry| entry.name.to_lowercase());

    Ok(DirectoryListing {
        path: canonical.to_string_lossy().into_owned(),
        parent: parent_directory(&canonical),
        entries,
        ..Default::default()
    })
}

/// Parent directory path. Empty at the filesystem root.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn parent_directory(path: &Path) -> String {
    path.parent()
        .map(|parent| parent.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// True when the suffix of `path` is one of the requested Cadmus extensions.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn extension_allowed(path: &Path, extensions: &[FileExtension]) -> bool {
    if extensions.is_empty() {
        return false;
    }
    path_file_extension(path).is_some_and(|file_ext| extensions.contains(&file_ext))
}

/// Rejects `path` when it is outside the device's file root.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
async fn confine(root: Option<&Path>, path: &Path) -> Result<(), ConnectError> {
    let Some(root) = root else {
        return Ok(());
    };
    let root = tokio::fs::canonicalize(root)
        .await
        .map_err(|_| ConnectError::unavailable("file root unavailable"))?;
    if path.starts_with(&root) {
        Ok(())
    } else {
        Err(ConnectError::permission_denied(
            "path outside the allowed root",
        ))
    }
}

/// Bytes of any regular file, at most 32 MiB.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(path)))]
async fn get_device_file(root: Option<&Path>, path: &str) -> Result<FileContents, ConnectError> {
    if path.trim().is_empty() {
        return Err(ConnectError::invalid_argument("invalid path"));
    }
    let canonical = match tokio::fs::canonicalize(path).await {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ConnectError::not_found("file not found"));
        }
        Err(_) => return Err(ConnectError::invalid_argument("invalid path")),
    };
    confine(root, &canonical).await?;
    let metadata = match tokio::fs::metadata(&canonical).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ConnectError::not_found("file not found"));
        }
        Err(_) => return Err(ConnectError::invalid_argument("invalid path")),
    };
    if !metadata.is_file() {
        return Err(ConnectError::invalid_argument("path is not a file"));
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err(ConnectError::resource_exhausted("file too large"));
    }
    let content = tokio::fs::read(&canonical)
        .await
        .map_err(|_| ConnectError::not_found("file not found"))?;
    Ok(FileContents {
        content,
        content_type: super::serve::content_type(&canonical).to_string(),
        ..Default::default()
    })
}

/// Cadmus [`FileExtension`] for this path, if the suffix is known.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn path_file_extension(path: &Path) -> Option<FileExtension> {
    path.extension()
        .and_then(|ext| ext.to_str())
        .and_then(|ext| ext.to_ascii_lowercase().parse().ok())
}

#[cfg(test)]
mod tests {
    use super::{Extension, FileExtension, get_device_file, list_device_directory};

    #[tokio::test]
    async fn list_directory_empty_path_lists_dirs_and_rejects_files() {
        let listing = list_device_directory(None, "", &[])
            .await
            .expect("current directory lists");
        assert!(!listing.path.is_empty());
        assert!(listing.entries.iter().all(|entry| entry.directory));
        let names: Vec<_> = listing
            .entries
            .iter()
            .map(|entry| entry.name.to_lowercase())
            .collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);

        let file = tempfile::NamedTempFile::new().unwrap();
        let error = list_device_directory(None, file.path().to_str().unwrap(), &[])
            .await
            .expect_err("file path is invalid");
        assert_eq!(
            error.code,
            connectrpc::ErrorCode::InvalidArgument,
            "{error}"
        );
    }

    #[tokio::test]
    async fn list_directory_with_extensions_includes_matching_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("subdir")).unwrap();
        std::fs::write(dir.path().join("photo.png"), b"png-bytes").unwrap();
        std::fs::write(dir.path().join("book.epub"), b"epub-bytes").unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"nope").unwrap();

        let listing = list_device_directory(
            None,
            dir.path().to_str().unwrap(),
            &[FileExtension::Png, FileExtension::Epub],
        )
        .await
        .expect("directory lists");

        let dirs: Vec<_> = listing
            .entries
            .iter()
            .filter(|entry| entry.directory)
            .map(|entry| entry.name.as_str())
            .collect();
        let mut files: Vec<_> = listing
            .entries
            .iter()
            .filter(|entry| !entry.directory)
            .map(|entry| entry.name.as_str())
            .collect();
        files.sort();
        assert!(dirs.contains(&"subdir"));
        assert_eq!(files, ["book.epub", "photo.png"]);
        assert_eq!(
            listing.parent,
            dir.path()
                .parent()
                .map(|parent| parent.to_string_lossy().into_owned())
                .unwrap_or_default()
        );
    }

    #[tokio::test]
    async fn get_file_returns_any_regular_file_and_rejects_missing_paths() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("photo.png");
        std::fs::write(&png, b"png-bytes").unwrap();
        let epub = dir.path().join("book.epub");
        std::fs::write(&epub, b"epub-bytes").unwrap();

        let file = get_device_file(None, png.to_str().unwrap())
            .await
            .expect("png reads");
        assert_eq!(file.content, b"png-bytes");
        assert_eq!(file.content_type, "image/png");

        let book = get_device_file(None, epub.to_str().unwrap())
            .await
            .expect("epub reads");
        assert_eq!(book.content, b"epub-bytes");
        assert_eq!(book.content_type, "application/epub+zip");

        let missing = dir.path().join("missing.png");
        let missing_error = get_device_file(None, missing.to_str().unwrap())
            .await
            .expect_err("missing rejected");
        assert_eq!(
            missing_error.code,
            connectrpc::ErrorCode::NotFound,
            "{missing_error}"
        );
    }

    #[tokio::test]
    async fn file_root_rejects_paths_outside_it() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("inside.txt"), b"inside").unwrap();
        std::fs::write(outside.path().join("outside.txt"), b"outside").unwrap();

        let inside = get_device_file(
            Some(root.path()),
            root.path().join("inside.txt").to_str().unwrap(),
        )
        .await
        .expect("inside the root");
        assert_eq!(inside.content, b"inside");

        let error = get_device_file(
            Some(root.path()),
            outside.path().join("outside.txt").to_str().unwrap(),
        )
        .await
        .expect_err("outside the root");
        assert_eq!(
            error.code,
            connectrpc::ErrorCode::PermissionDenied,
            "{error}"
        );
    }

    #[test]
    fn proto_extension_maps_every_file_extension() {
        for ext in FileExtension::all() {
            let proto = Extension::from(*ext);
            assert_ne!(proto, Extension::EXTENSION_UNSPECIFIED);
            assert_eq!(FileExtension::try_from(proto).expect("mapped"), *ext);
        }
    }
}
