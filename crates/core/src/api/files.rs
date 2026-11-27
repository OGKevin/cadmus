//! Device filesystem RPCs shared by public and private listeners.

use super::handler::PublicRpc;
use super::serve::FILE_CHUNK_BYTES;
use crate::document::file_extension::{FileExtension, UnknownFileExtension};
use cadmus_api_proto::cadmus::api::shared::v1::{
    DirectoryEntry, DirectoryListing, Extension, FileChunk, FileOps, GetFileRequest,
    ListDirectoryRequest,
};
use connectrpc::{
    ConnectError, Encodable, RequestContext, ServiceRequest, ServiceResult, ServiceStream,
};
use std::future::Future;
use std::path::{Path, PathBuf};
use tokio::io::AsyncReadExt;
use tokio::sync::OwnedSemaphorePermit;

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
    fn get_file(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, GetFileRequest>,
    ) -> impl Future<Output = ServiceResult<ServiceStream<impl Encodable<FileChunk> + Send + use<>>>>
    + Send {
        let owned = request.to_owned_message();
        let file_root = self.file_root.clone();
        let file_limit = std::sync::Arc::clone(&self.file_limit);
        async move {
            let permit = file_limit
                .acquire_owned()
                .await
                .map_err(|_| ConnectError::unavailable("api stopped"))?;
            file_chunks(
                file_root.as_deref().map(PathBuf::as_path),
                &owned.path,
                permit,
            )
            .await
            .and_then(connectrpc::Response::stream_ok)
        }
    }
}

/// Lists immediate children of `path`.
///
/// An empty `path` starts at the listener file root when one is set, otherwise
/// at the process current directory. An empty `extensions` list returns
/// directories only. Otherwise directories and files whose suffix matches one
/// of `extensions` are included.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(path = path, extensions = extensions.len())))]
async fn list_device_directory(
    root: Option<&Path>,
    path: &str,
    extensions: &[FileExtension],
) -> Result<DirectoryListing, ConnectError> {
    let requested = if path.trim().is_empty() {
        root.map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    } else {
        PathBuf::from(path)
    };
    let canonical = match tokio::fs::canonicalize(&requested).await {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ConnectError::not_found("file not found"));
        }
        Err(_) => return Err(ConnectError::invalid_argument("invalid path")),
    };
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
    entries.sort_by_key(|entry| (!entry.directory, entry.name.to_lowercase()));

    Ok(DirectoryListing {
        path: canonical.to_string_lossy().into_owned(),
        parent: parent_within(root, &canonical).await,
        entries,
        ..Default::default()
    })
}

/// Parent path, empty when `path` is the file root or the filesystem root.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
async fn parent_within(root: Option<&Path>, path: &Path) -> String {
    if let Some(root) = root
        && let Ok(root) = tokio::fs::canonicalize(root).await
        && path == root
    {
        return String::new();
    }
    parent_directory(path)
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
        Err(ConnectError::not_found("file not found"))
    }
}

/// Read position of [`file_chunks`].
enum ChunkState {
    /// Reads continue; `content_type` is still pending for the first chunk.
    Reading {
        file: tokio::fs::File,
        remaining: u64,
        content_type: Option<&'static str>,
        _permit: OwnedSemaphorePermit,
    },
    /// The stream is over; the read permit is released.
    Done,
}

/// Streams a regular file in [`FILE_CHUNK_BYTES`]-sized [`FileChunk`]s, at most 32 MiB.
///
/// The read permit lives in the stream state, so the concurrency cap holds for
/// the whole transfer until EOF or the client drops the stream. A zero-byte
/// file yields one empty chunk carrying its content type, and a file that grows
/// past the cap ends the stream with `resource_exhausted` instead of silently
/// truncating.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(path = path)))]
async fn file_chunks(
    root: Option<&Path>,
    path: &str,
    permit: OwnedSemaphorePermit,
) -> Result<ServiceStream<FileChunk>, ConnectError> {
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
    let file = tokio::fs::File::open(&canonical)
        .await
        .map_err(|_| ConnectError::not_found("file not found"))?;
    let stream = futures_lite::stream::unfold(
        ChunkState::Reading {
            file,
            remaining: MAX_FILE_BYTES,
            content_type: Some(super::serve::content_type(&canonical)),
            _permit: permit,
        },
        |state| async move {
            let (mut file, remaining, content_type, _permit) = match state {
                ChunkState::Reading {
                    file,
                    remaining,
                    content_type,
                    _permit,
                } => (file, remaining, content_type, _permit),
                ChunkState::Done => return None,
            };
            let read_limit = if remaining == 0 {
                1
            } else {
                remaining.min(FILE_CHUNK_BYTES as u64) as usize
            };
            let mut data = vec![0u8; read_limit];
            match AsyncReadExt::read(&mut file, &mut data).await {
                Ok(0) => {
                    let content_type = content_type?;
                    Some((
                        Ok(FileChunk {
                            content_type: content_type.to_string(),
                            ..Default::default()
                        }),
                        ChunkState::Done,
                    ))
                }
                Ok(_) if remaining == 0 => {
                    tracing::warn!("api file grew past the size cap");
                    Some((
                        Err(ConnectError::resource_exhausted("file too large")),
                        ChunkState::Done,
                    ))
                }
                Ok(read) => {
                    data.truncate(read);
                    Some((
                        Ok(FileChunk {
                            data,
                            content_type: content_type.unwrap_or_default().to_string(),
                            ..Default::default()
                        }),
                        ChunkState::Reading {
                            file,
                            remaining: remaining - read as u64,
                            content_type: None,
                            _permit,
                        },
                    ))
                }
                Err(error) => {
                    tracing::warn!(error = %error, "api file read failed");
                    Some((
                        Err(ConnectError::internal("file read failed")),
                        ChunkState::Done,
                    ))
                }
            }
        },
    );
    Ok(Box::pin(stream))
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
    use super::{Extension, FileExtension, file_chunks, list_device_directory};
    use crate::api::serve::FILE_CHUNK_BYTES;
    use futures_lite::StreamExt as _;
    use std::path::Path;
    use std::sync::Arc;
    use tokio::sync::Semaphore;

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
    async fn list_directory_empty_path_starts_at_the_file_root() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("shelf")).unwrap();
        let listing = list_device_directory(Some(root.path()), "", &[])
            .await
            .expect("root lists");
        let root = std::fs::canonicalize(root.path()).unwrap();
        assert_eq!(Path::new(&listing.path), root.as_path());
        assert!(listing.entries.iter().any(|entry| entry.name == "shelf"));
        assert_eq!(listing.parent, "");
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
            listing.entries.first().map(|entry| entry.directory),
            Some(true),
            "directories sort before files"
        );
        assert_eq!(
            listing.parent,
            dir.path()
                .parent()
                .map(|parent| parent.to_string_lossy().into_owned())
                .unwrap_or_default()
        );
    }

    /// Drains every chunk [`file_chunks`] yields for `path`.
    async fn collected_file(
        root: Option<&Path>,
        path: &str,
    ) -> Result<(Vec<u8>, String), connectrpc::ConnectError> {
        let limit = Arc::new(Semaphore::new(1));
        let permit = Arc::clone(&limit).acquire_owned().await.unwrap();
        let mut stream = file_chunks(root, path, permit).await?;
        let mut content = Vec::new();
        let mut content_type = String::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            if content_type.is_empty() {
                content_type = chunk.content_type;
            }
            content.extend_from_slice(&chunk.data);
        }
        Ok((content, content_type))
    }

    #[tokio::test]
    async fn get_file_returns_any_regular_file_and_rejects_missing_paths() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("photo.png");
        std::fs::write(&png, b"png-bytes").unwrap();
        let epub = dir.path().join("book.epub");
        std::fs::write(&epub, b"epub-bytes").unwrap();

        let (content, content_type) = collected_file(None, png.to_str().unwrap())
            .await
            .expect("png reads");
        assert_eq!(content, b"png-bytes");
        assert_eq!(content_type, "image/png");

        let (content, content_type) = collected_file(None, epub.to_str().unwrap())
            .await
            .expect("epub reads");
        assert_eq!(content, b"epub-bytes");
        assert_eq!(content_type, "application/epub+zip");

        let missing = dir.path().join("missing.png");
        let missing_error = collected_file(None, missing.to_str().unwrap())
            .await
            .expect_err("missing rejected");
        assert_eq!(
            missing_error.code,
            connectrpc::ErrorCode::NotFound,
            "{missing_error}"
        );
    }

    #[tokio::test]
    async fn get_file_streams_large_files_across_chunk_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.bin");
        let content: Vec<u8> = (0..(FILE_CHUNK_BYTES * 2 + 17))
            .map(|index| (index % 251) as u8)
            .collect();
        std::fs::write(&path, &content).unwrap();
        let (bytes, _) = collected_file(None, path.to_str().unwrap())
            .await
            .expect("stream reads");
        assert_eq!(bytes, content);
    }

    #[tokio::test]
    async fn get_file_reports_the_content_type_of_empty_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.epub");
        std::fs::write(&path, b"").unwrap();
        let limit = Arc::new(Semaphore::new(1));
        let permit = Arc::clone(&limit).acquire_owned().await.unwrap();
        let mut stream = file_chunks(None, path.to_str().unwrap(), permit)
            .await
            .expect("stream opens");
        let chunk = stream
            .next()
            .await
            .expect("one metadata chunk")
            .expect("metadata chunk is ok");
        assert!(chunk.data.is_empty());
        assert_eq!(chunk.content_type, "application/epub+zip");
        assert!(
            stream.next().await.is_none(),
            "the stream ends after the metadata chunk"
        );
    }

    #[tokio::test]
    async fn get_file_holds_its_read_permit_until_the_stream_ends() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.epub");
        std::fs::write(&path, b"epub-bytes").unwrap();
        let limit = Arc::new(Semaphore::new(1));
        let permit = Arc::clone(&limit).acquire_owned().await.unwrap();
        let mut stream = file_chunks(None, path.to_str().unwrap(), permit)
            .await
            .expect("stream opens");
        assert!(
            Arc::clone(&limit).try_acquire_owned().is_err(),
            "the open stream holds the read permit"
        );
        while stream.next().await.is_some() {}
        assert!(
            Arc::clone(&limit).try_acquire_owned().is_ok(),
            "the permit returns after the stream ends"
        );
    }

    #[tokio::test]
    async fn file_root_rejects_paths_outside_it() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("inside.txt"), b"inside").unwrap();
        std::fs::write(outside.path().join("outside.txt"), b"outside").unwrap();

        let (inside, _) = collected_file(
            Some(root.path()),
            root.path().join("inside.txt").to_str().unwrap(),
        )
        .await
        .expect("inside the root");
        assert_eq!(inside, b"inside");

        let error = collected_file(
            Some(root.path()),
            outside.path().join("outside.txt").to_str().unwrap(),
        )
        .await
        .expect_err("outside the root");
        assert_eq!(error.code, connectrpc::ErrorCode::NotFound, "{error}");
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
