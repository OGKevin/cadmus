//! The Log bundle: the selected logs in one zip.
//!
//! Written as a stream, so a log of a gigabyte needs no more memory than the
//! deflate window and the output chunks. Download sends the bytes to the
//! browser as they are made.
//!
//! The writer compresses on the blocking pool and emits zip bytes through the
//! caller's sink, which forwards them as they are made. `async_zip`'s streaming
//! pre-compressed entries cannot be used here: they need the CRC32 of the
//! whole file in the entry before any compressed data is written, so computing
//! that CRC while compressing would mean buffering a whole log or reading it
//! twice. `zip` writes non-seekable archives with data descriptors and computes
//! the CRC as the bytes go by.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

/// Deflate level for bundles built on the device.
///
/// Level 6 costs about twice the time of level 1 for a few percent smaller
/// bundles. The device rule is to keep it while it compresses at 10 MiB/s or
/// more.
pub(super) const DEFLATE_LEVEL: i64 = 6;

/// A log to put in a bundle.
#[derive(Clone, Debug)]
pub(super) struct BundleFile {
    /// Name inside the archive.
    pub(super) name: String,
    pub(super) path: PathBuf,
    /// Bytes to take: the size when the bundle was requested. A log that grew
    /// since is cut there, and one that shrank fails the bundle.
    pub(super) size: u64,
    /// Modification time, seconds since the Unix epoch.
    pub(super) modified: u64,
}

/// Reads exactly `remaining` bytes and counts them, or fails.
struct Exact<'a> {
    inner: File,
    remaining: u64,
    read: &'a AtomicU64,
}

impl Read for Exact<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return Ok(0);
        }
        let limit = (buf.len() as u64).min(self.remaining) as usize;
        let read = self.inner.read(&mut buf[..limit])?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "a log became shorter while it was bundled",
            ));
        }
        self.remaining -= read as u64;
        self.read.fetch_add(read as u64, Ordering::Relaxed);
        Ok(read)
    }
}

/// Writes `files` as a deflated zip at the device level into `sink` and
/// returns the sink.
///
/// `read` counts the log bytes taken so far, for progress.
///
/// # Errors
///
/// Returns the I/O error of a log or of the sink. A sink error ends the bundle
/// at once.
pub(super) fn write_bundle<W: Write>(
    files: &[BundleFile],
    sink: W,
    read: &AtomicU64,
) -> io::Result<W> {
    write_bundle_with(files, sink, read, DEFLATE_LEVEL)
}

/// [`write_bundle`] with an explicit `deflate_level`, so the benchmark can
/// compare settings.
///
/// # Errors
///
/// Returns the I/O error of a log or of the sink.
pub(super) fn write_bundle_with<W: Write>(
    files: &[BundleFile],
    sink: W,
    read: &AtomicU64,
    deflate_level: i64,
) -> io::Result<W> {
    let mut archive = ZipWriter::new_stream(sink);
    for file in files {
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(Some(deflate_level))
            .unix_permissions(0o644)
            .last_modified_time(modified_time(file.modified));
        archive.start_file(&file.name, options).map_err(zip_error)?;
        let mut reader = Exact {
            inner: File::open(&file.path)?,
            remaining: file.size,
            read,
        };
        io::copy(&mut reader, &mut archive)?;
    }
    archive
        .finish()
        .map(zip::write::StreamWriter::into_inner)
        .map_err(zip_error)
}

/// The zip timestamp for `seconds` since the Unix epoch, or now when it falls
/// outside the DOS date range.
fn modified_time(seconds: u64) -> DateTime {
    i64::try_from(seconds)
        .ok()
        .and_then(|seconds| chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, 0))
        .and_then(|time| DateTime::try_from(time.naive_utc()).ok())
        .unwrap_or_else(DateTime::default_for_write)
}

/// Keeps an I/O error's kind, which the download relies on to tell a client
/// that went away from a real failure.
fn zip_error(error: zip::result::ZipError) -> io::Error {
    match error {
        zip::result::ZipError::Io(error) => error,
        error => io::Error::other(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn log(dir: &std::path::Path, name: &str, body: &[u8]) -> BundleFile {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        BundleFile {
            name: name.to_owned(),
            path,
            size: body.len() as u64,
            modified: 1_700_000_000,
        }
    }

    fn unpack(bundle: Vec<u8>) -> Vec<(String, Vec<u8>)> {
        let mut archive = zip::ZipArchive::new(Cursor::new(bundle)).unwrap();
        (0..archive.len())
            .map(|index| {
                let mut entry = archive.by_index(index).unwrap();
                let name = entry.name().unwrap().into_owned();
                let mut body = Vec::new();
                entry.read_to_end(&mut body).unwrap();
                (name, body)
            })
            .collect()
    }

    #[test]
    fn the_bundle_unzips_to_the_selected_logs_under_their_names() {
        let dir = tempfile::tempdir().unwrap();
        let big = "line of a Run log\n".repeat(50_000);
        let files = [
            log(dir.path(), "cadmus-a.json", big.as_bytes()),
            log(dir.path(), "info.log", b"launcher\n"),
        ];
        let read = AtomicU64::new(0);
        let bundle = write_bundle(&files, Vec::new(), &read).unwrap();
        assert!(bundle.len() < big.len() / 10, "text must compress");
        assert_eq!(
            unpack(bundle),
            [
                ("cadmus-a.json".to_owned(), big.into_bytes()),
                ("info.log".to_owned(), b"launcher\n".to_vec()),
            ]
        );
        assert_eq!(read.load(Ordering::Relaxed), 900_000 + 9);
    }

    #[test]
    fn a_level_changes_the_bundle_size() {
        let dir = tempfile::tempdir().unwrap();
        let body = "line of a Run log\n".repeat(50_000);
        let file = log(dir.path(), "cadmus-a.json", body.as_bytes());
        let fast = write_bundle_with(
            std::slice::from_ref(&file),
            Vec::new(),
            &AtomicU64::new(0),
            1,
        )
        .unwrap();
        let small = write_bundle_with(
            std::slice::from_ref(&file),
            Vec::new(),
            &AtomicU64::new(0),
            DEFLATE_LEVEL,
        )
        .unwrap();
        assert!(small.len() <= fast.len());
    }

    #[test]
    fn a_log_that_grew_is_cut_at_the_requested_size() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = log(dir.path(), "cadmus-a.json", b"first\nsecond\n");
        file.size = 6;
        let bundle = write_bundle(&[file], Vec::new(), &AtomicU64::new(0)).unwrap();
        assert_eq!(
            unpack(bundle),
            [("cadmus-a.json".to_owned(), b"first\n".to_vec())]
        );
    }

    #[test]
    fn a_log_that_shrank_fails_the_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = log(dir.path(), "cadmus-a.json", b"short\n");
        file.size = 100;
        let error = write_bundle(&[file], Vec::new(), &AtomicU64::new(0)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn a_sink_that_fails_ends_the_bundle() {
        #[derive(Debug)]
        struct Closed;
        impl Write for Closed {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "gone"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let noise: Vec<u8> = (0..4_000_000u32)
            .map(|n| (n.wrapping_mul(2_654_435_761) >> 24) as u8)
            .collect();
        let file = log(dir.path(), "cadmus-a.json", &noise);
        let error = write_bundle(&[file], Closed, &AtomicU64::new(0)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }
}
