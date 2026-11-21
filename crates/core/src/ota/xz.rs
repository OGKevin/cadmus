//! xz payload handling for OTA updates.
//!
//! CI builds and stable releases ship an xz copy of the `KoboRoot` bundle.
//! Nickel can only install a gzip `KoboRoot.tgz`, so the device decodes the xz
//! stream and re-compresses it into the staging file in a single pass. Neither
//! the compressed payload nor the decompressed tar is ever written to flash by
//! this module; the only output is the gzip stream written to the staging file.
//!
//! Reads and writes are async. The codec is sans-I/O and CPU bound, so each
//! chunk of compressed input is handed to the blocking pool and the gzip bytes
//! it produced come back to be written. Cancellation and progress are handled
//! between chunks on the async side.

use super::client::{OtaError, open_kobo_root_entry, zip_body_error};
use crate::github::OtaProgress;
use crate::http::CancelFunc;
use async_zip::base::read1::seek::ZipArchiveReader;
use futures_lite::io::{AsyncBufRead, AsyncRead, AsyncReadExt, AsyncSeek, Cursor};
use lzma_rust2::{Action, Status, XzStream};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio_util::compat::TokioAsyncReadCompatExt;

/// RAM held on top of the compressed payload while it is decoded: the 16 MiB
/// dictionary, I/O buffers and gzip encoder state.
pub(super) const XZ_DECODE_RESERVE: u64 = 32 * 1024 * 1024;

/// Upper bound on decoder memory. A hostile stream asking for a bigger
/// dictionary is rejected instead of exhausting the device.
const XZ_DECODER_MEM_LIMIT_KIB: u32 = 64 * 1024;

/// Compressed bytes handed to the codec per blocking-pool round trip.
const INPUT_CHUNK_SIZE: usize = 256 * 1024;

/// Decoded bytes the codec produces per call before they are re-compressed.
const DECODE_BUFFER_SIZE: usize = 256 * 1024;

/// Gzip bytes one codec step may hold before it returns them to be written.
/// Highly repetitive xz input can decode to far more than its own size, so
/// output is drained in bounded steps rather than once per input chunk.
const STEP_OUTPUT_LIMIT: usize = 1024 * 1024;

/// Name of the xz bundle inside a CI artifact ZIP.
pub(super) fn kobo_root_xz_name() -> &'static str {
    cfg_select! {
        feature = "test" => { "KoboRoot-test.tar.xz" }
        _ => { "KoboRoot.tar.xz" }
    }
}

/// Whether an xz payload of `payload_size` bytes can be held in RAM while it is
/// decoded.
pub(super) fn fits_in_memory(payload_size: u64) -> bool {
    matches!(
        crate::memory::choose_buffer_path(payload_size.saturating_add(XZ_DECODE_RESERVE)),
        crate::memory::BufferPath::InMemory
    )
}

/// Largest gzip stream that `raw_size` bytes of input can produce.
///
/// Uses zlib's worst-case `compressBound` formula plus the 18-byte gzip header
/// and trailer, so it holds for any content, including data that does not
/// compress at all.
pub(super) fn gzip_bound(raw_size: u64) -> u64 {
    raw_size
        .saturating_add(raw_size >> 12)
        .saturating_add(raw_size >> 14)
        .saturating_add(raw_size >> 25)
        .saturating_add(13 + 18)
}

/// Where a downloaded xz payload lives before it is decoded.
#[derive(Debug)]
pub enum XzSource {
    /// A CI artifact ZIP wrapping `KoboRoot.tar.xz`, held in memory.
    ZipBytes(Vec<u8>),
    /// A CI artifact ZIP wrapping `KoboRoot.tar.xz`, on disk.
    ZipFile(PathBuf),
    /// A bare `KoboRoot.tar.xz` release asset, held in memory.
    RawBytes(Vec<u8>),
    /// A bare `KoboRoot.tar.xz` release asset, on disk.
    RawFile(PathBuf),
}

impl XzSource {
    /// Path of the on-disk payload, if there is one.
    pub(super) fn path(&self) -> Option<&Path> {
        match self {
            Self::ZipFile(path) | Self::RawFile(path) => Some(path),
            Self::ZipBytes(_) | Self::RawBytes(_) => None,
        }
    }
}

/// Size of the decoded tar, read from the xz index without decoding anything.
///
/// A ZIP source needs its member stored rather than deflated so the xz footer
/// can be reached by seeking; CI uploads with `compression-level: 0` for that
/// reason. Returns `None` when the size cannot be read, which only skips the
/// space check.
pub(super) async fn uncompressed_size(source: &XzSource) -> Option<u64> {
    let result = match source {
        XzSource::RawBytes(bytes) => xz_index_size(std::io::Cursor::new(bytes.as_slice())),
        XzSource::ZipBytes(bytes) => zip_member_index_size(std::io::Cursor::new(bytes.as_slice())),
        XzSource::RawFile(path) => {
            let path = path.clone();
            index_size_off_thread(move || xz_index_size(std::fs::File::open(path)?)).await
        }
        XzSource::ZipFile(path) => {
            let path = path.clone();
            index_size_off_thread(move || {
                zip_member_index_size(std::io::BufReader::new(std::fs::File::open(path)?))
            })
            .await
        }
    };

    match result {
        Ok(size) => Some(size),
        Err(error) => {
            tracing::warn!(error = %error, "Could not read the xz index");
            None
        }
    }
}

/// Runs a small synchronous index read on the blocking pool.
async fn index_size_off_thread<F>(read: F) -> std::io::Result<u64>
where
    F: FnOnce() -> std::io::Result<u64> + Send + 'static,
{
    crate::runtime::spawn_blocking(read)
        .await
        .unwrap_or_else(|error| Err(std::io::Error::other(error)))
}

fn xz_index_size<R: Read + Seek>(reader: R) -> std::io::Result<u64> {
    lzma_rust2::xz_uncompressed_size(reader)
}

fn zip_member_index_size<R: Read + Seek>(reader: R) -> std::io::Result<u64> {
    let mut archive = zip::ZipArchive::new(reader).map_err(std::io::Error::other)?;
    let name = kobo_root_xz_name();
    let index = (0..archive.len())
        .find(|&index| {
            archive
                .name_for_index(index)
                .and_then(Result::ok)
                .is_some_and(|entry| {
                    !entry.ends_with('/')
                        && Path::new(entry.as_ref())
                            .file_name()
                            .and_then(|n| n.to_str())
                            == Some(name)
                })
        })
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("{name} not in ZIP"))
        })?;
    let member = archive
        .by_index_seek(index)
        .map_err(std::io::Error::other)?;
    xz_index_size(member)
}

/// Result of one codec step.
struct Step {
    /// Gzip bytes produced by this step.
    gzip: Vec<u8>,
    /// Compressed input bytes consumed by this step.
    consumed: usize,
    /// The xz input has ended.
    finished: bool,
    /// The decoder needs more input; calling again with the same input does
    /// nothing.
    idle: bool,
}

/// The sans-I/O codec: xz in, gzip out.
struct Transcoder {
    stream: XzStream,
    encoder: flate2::write::GzEncoder<Vec<u8>>,
    decoded: Vec<u8>,
}

impl Transcoder {
    /// Concatenated xz streams and stream padding are decoded through EOF, so
    /// a multi-stream payload is never silently cut short.
    fn new() -> Self {
        Self {
            stream: XzStream::new_mem_limit(true, XZ_DECODER_MEM_LIMIT_KIB),
            encoder: flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()),
            decoded: vec![0; DECODE_BUFFER_SIZE],
        }
    }

    /// Decodes `input` until it is used up, the xz input ends, or
    /// [`STEP_OUTPUT_LIMIT`] gzip bytes are pending.
    ///
    /// With [`Action::Finish`] and no more input, an xz stream that has not
    /// ended is reported as truncated by the decoder.
    fn feed(&mut self, input: &[u8], action: Action) -> Result<Step, OtaError> {
        let mut position = 0;
        let (finished, idle) = loop {
            let result = self
                .stream
                .process(&input[position..], &mut self.decoded, action)
                .map_err(|error| OtaError::Decompress(error.to_string()))?;
            position += result.bytes_consumed;
            self.encoder
                .write_all(&self.decoded[..result.bytes_produced])?;

            if result.status == Status::StreamEnd {
                break (true, false);
            }
            if result.bytes_consumed == 0 && result.bytes_produced == 0 {
                break (false, true);
            }
            if self.encoder.get_ref().len() >= STEP_OUTPUT_LIMIT {
                break (false, false);
            }
        };
        Ok(Step {
            gzip: std::mem::take(self.encoder.get_mut()),
            consumed: position,
            finished,
            idle,
        })
    }

    fn finish(self) -> Result<Vec<u8>, OtaError> {
        Ok(self.encoder.finish()?)
    }
}

/// Moves the codec and its input buffer to the blocking pool for one step and
/// gets them back with the result.
async fn step_off_thread(
    mut transcoder: Transcoder,
    input: Vec<u8>,
    range: std::ops::Range<usize>,
    action: Action,
) -> Result<(Transcoder, Vec<u8>, Result<Step, OtaError>), OtaError> {
    crate::runtime::spawn_blocking(move || {
        let step = transcoder.feed(&input[range], action);
        (transcoder, input, step)
    })
    .await
    .map_err(|error| OtaError::Io(std::io::Error::other(error)))
}

/// Runs codec steps over `input[..len]` until it is used up or the xz input
/// ends, writing each step's gzip output before the next step runs.
///
/// Returns the codec, the input buffer and whether the xz input has ended.
async fn pump<W>(
    mut transcoder: Transcoder,
    mut input: Vec<u8>,
    len: usize,
    action: Action,
    out: &mut W,
    should_cancel: CancelFunc<'_>,
) -> Result<(Transcoder, Vec<u8>, bool), OtaError>
where
    W: AsyncWrite + Unpin,
{
    let mut start = 0;
    loop {
        if should_cancel.is_cancelled() {
            return Err(OtaError::Cancelled);
        }
        let (next, returned, step) = step_off_thread(transcoder, input, start..len, action).await?;
        transcoder = next;
        input = returned;
        let step = step?;
        out.write_all(&step.gzip).await?;
        start += step.consumed;
        if step.finished || step.idle {
            return Ok((transcoder, input, step.finished));
        }
    }
}

/// Decompression progress is reported in steps of this many percent so screen
/// refreshes do not throttle decoding.
const PROGRESS_STEP_PERCENT: u64 = 10;

/// Decodes `source` and writes it to `out` as a gzip stream.
///
/// `progress` receives [`OtaProgress::Decompressing`] each time the
/// consumed share of compressed bytes crosses a 10 % step, ending at 100 %.
///
/// # Errors
///
/// * [`OtaError::Cancelled`] - `should_cancel` tripped between chunks
/// * [`OtaError::Decompress`] - the xz stream is corrupt, truncated, fails its
///   integrity check or needs more memory than the decoder allows
/// * [`OtaError::ZipError`] - the wrapping ZIP is invalid or fails its CRC32
/// * [`OtaError::DeploymentError`] - the ZIP has no `KoboRoot` xz member
/// * [`OtaError::Io`] - the source cannot be read or `out` cannot be written
pub(super) async fn transcode_to_gzip<W, P>(
    source: &XzSource,
    out: &mut W,
    should_cancel: CancelFunc<'_>,
    progress: &mut P,
) -> Result<(), OtaError>
where
    W: AsyncWrite + Unpin,
    P: FnMut(OtaProgress),
{
    match source {
        XzSource::ZipBytes(bytes) => {
            let mut archive = ZipArchiveReader::open(Cursor::new(bytes.as_slice()))
                .await
                .map_err(|error| OtaError::ZipError(error.to_string()))?;
            transcode_zip_member(&mut archive, out, should_cancel, progress).await
        }
        XzSource::ZipFile(path) => {
            let file = tokio::fs::File::open(path).await?;
            let mut archive = ZipArchiveReader::open(tokio::io::BufReader::new(file).compat())
                .await
                .map_err(|error| OtaError::ZipError(error.to_string()))?;
            transcode_zip_member(&mut archive, out, should_cancel, progress).await
        }
        XzSource::RawBytes(bytes) => {
            let total = bytes.len() as u64;
            transcode_stream(
                Cursor::new(bytes.as_slice()),
                total,
                raw_read_error,
                out,
                should_cancel,
                progress,
            )
            .await
        }
        XzSource::RawFile(path) => {
            let file = tokio::fs::File::open(path).await?;
            let total = file.metadata().await?.len();
            transcode_stream(
                tokio::io::BufReader::new(file).compat(),
                total,
                raw_read_error,
                out,
                should_cancel,
                progress,
            )
            .await
        }
    }
}

fn raw_read_error(error: &std::io::Error) -> OtaError {
    OtaError::Io(std::io::Error::new(error.kind(), error.to_string()))
}

fn zip_read_error(error: &std::io::Error) -> OtaError {
    OtaError::ZipError(zip_body_error(error))
}

async fn transcode_zip_member<R, W, P>(
    archive: &mut ZipArchiveReader<R>,
    out: &mut W,
    should_cancel: CancelFunc<'_>,
    progress: &mut P,
) -> Result<(), OtaError>
where
    R: AsyncBufRead + AsyncSeek + Unpin,
    W: AsyncWrite + Unpin,
    P: FnMut(OtaProgress),
{
    let (entry, total) = open_kobo_root_entry(archive, kobo_root_xz_name(), should_cancel).await?;
    transcode_stream(entry, total, zip_read_error, out, should_cancel, progress).await
}

/// Pumps `reader` through the codec into `out`.
///
/// Reading continues to the end of the source after the xz stream ends, so a
/// wrapping ZIP member has its own CRC32 verified.
async fn transcode_stream<R, W, P>(
    mut reader: R,
    total: u64,
    read_error: fn(&std::io::Error) -> OtaError,
    out: &mut W,
    should_cancel: CancelFunc<'_>,
    progress: &mut P,
) -> Result<(), OtaError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    P: FnMut(OtaProgress),
{
    let mut transcoder = Transcoder::new();
    let mut buffer = vec![0_u8; INPUT_CHUNK_SIZE];
    let mut consumed = 0_u64;
    let mut finished = false;
    let mut last_percent = None;
    let divisor = total.max(1);

    loop {
        if should_cancel.is_cancelled() {
            return Err(OtaError::Cancelled);
        }

        let read = reader
            .read(&mut buffer)
            .await
            .map_err(|error| read_error(&error))?;
        if read == 0 {
            break;
        }
        consumed += read as u64;
        if finished {
            continue;
        }

        let (next, returned, ended) =
            pump(transcoder, buffer, read, Action::Run, out, should_cancel).await?;
        transcoder = next;
        buffer = returned;
        finished = ended;

        let done = consumed.min(total);
        let percent =
            done.saturating_mul(100) / divisor / PROGRESS_STEP_PERCENT * PROGRESS_STEP_PERCENT;
        if last_percent != Some(percent) {
            last_percent = Some(percent);
            progress(OtaProgress::Decompressing { done, total });
        }
    }

    if !finished {
        let (next, _, ended) = pump(
            transcoder,
            Vec::new(),
            0,
            Action::Finish,
            out,
            should_cancel,
        )
        .await?;
        transcoder = next;
        if !ended {
            return Err(OtaError::Decompress(
                "unexpected end of xz stream".to_owned(),
            ));
        }
    }

    out.write_all(&transcoder.finish()?).await?;
    out.flush().await?;

    if last_percent != Some(100) {
        progress(OtaProgress::Decompressing { done: total, total });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xz_bytes(data: &[u8]) -> Vec<u8> {
        let mut options = lzma_rust2::XzOptions::with_preset(6);
        options.lzma_options.dict_size = 16 << 20;
        let mut writer = lzma_rust2::XzWriter::new(Vec::new(), options).unwrap();
        writer.write_all(data).unwrap();
        writer.finish().unwrap()
    }

    fn zip_bytes(entry_name: &str, data: &[u8], method: zip::CompressionMethod) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default().compression_method(method);
        writer.start_file(entry_name, options).unwrap();
        writer.write_all(data).unwrap();
        writer.finish().unwrap().into_inner()
    }

    fn stored_zip(entry_name: &str, data: &[u8]) -> Vec<u8> {
        zip_bytes(entry_name, data, zip::CompressionMethod::Stored)
    }

    fn never_cancel() -> CancelFunc<'static> {
        CancelFunc::never()
    }

    fn gunzip(gz: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(gz), &mut out).unwrap();
        out
    }

    fn sample_tar() -> Vec<u8> {
        (0..300_000_u32)
            .flat_map(|n| n.wrapping_mul(2_654_435_761).to_le_bytes())
            .collect()
    }

    async fn transcode(source: &XzSource) -> Result<Vec<u8>, OtaError> {
        let mut out = Vec::new();
        transcode_to_gzip(source, &mut out, never_cancel(), &mut |_| {}).await?;
        Ok(out)
    }

    #[tokio::test]
    async fn concatenated_xz_streams_are_all_decoded() {
        let first = sample_tar();
        let second: Vec<u8> = first.iter().rev().copied().collect();
        let mut payload = xz_bytes(&first);
        payload.extend_from_slice(&[0; 4]);
        payload.extend_from_slice(&xz_bytes(&second));

        let out = transcode(&XzSource::RawBytes(payload)).await.unwrap();

        assert_eq!(gunzip(&out), [first, second].concat());
    }

    #[test]
    fn one_step_holds_at_most_the_output_limit() {
        let block = sample_tar();
        let tar = block.repeat(16);
        let xz = xz_bytes(&tar);
        let mut transcoder = Transcoder::new();
        let step = transcoder.feed(&xz, Action::Run).unwrap();

        assert!(!step.idle && !step.finished);
        assert!(step.gzip.len() < STEP_OUTPUT_LIMIT + DECODE_BUFFER_SIZE * 2);
    }

    #[tokio::test]
    async fn raw_bytes_round_trip_to_gzip_with_monotonic_progress() {
        let tar = sample_tar();
        let source = XzSource::RawBytes(xz_bytes(&tar));
        let mut out = Vec::new();
        let mut events = Vec::new();

        transcode_to_gzip(&source, &mut out, never_cancel(), &mut |event| {
            events.push(event)
        })
        .await
        .unwrap();

        assert_eq!(gunzip(&out), tar);

        let mut last_done = 0;
        for event in &events {
            let OtaProgress::Decompressing { done, total } = event else {
                panic!("unexpected event {event:?}");
            };
            assert!(*done >= last_done && *done <= *total);
            last_done = *done;
        }
        assert!(
            matches!(events.last(), Some(OtaProgress::Decompressing { done, total }) if done == total)
        );
        assert!(events.len() <= 11);
    }

    #[tokio::test]
    async fn raw_file_and_zip_sources_produce_the_same_gzip_payload() {
        let tar = sample_tar();
        let xz = xz_bytes(&tar);
        let dir = tempfile::tempdir().unwrap();

        let raw_path = dir.path().join("KoboRoot.tar.xz");
        std::fs::write(&raw_path, &xz).unwrap();
        let zip = stored_zip(kobo_root_xz_name(), &xz);
        let zip_path = dir.path().join("artifact.zip");
        std::fs::write(&zip_path, &zip).unwrap();

        for source in [
            XzSource::RawFile(raw_path),
            XzSource::ZipFile(zip_path),
            XzSource::ZipBytes(zip),
        ] {
            let out = transcode(&source).await.unwrap();
            assert_eq!(gunzip(&out), tar, "{source:?}");
        }
    }

    fn multi_block_xz_bytes(data: &[u8]) -> Vec<u8> {
        let mut options = lzma_rust2::XzOptions::with_preset(1);
        options.lzma_options.dict_size = 64 * 1024;
        options.block_size = std::num::NonZeroU64::new(64 * 1024);
        let mut writer = lzma_rust2::XzWriterMt::new(Vec::new(), options, 2).unwrap();
        writer.write_all(data).unwrap();
        writer.finish().unwrap()
    }

    #[tokio::test]
    async fn multi_block_xz_from_the_threaded_encoder_is_decoded_and_sized() {
        let tar = sample_tar();
        let xz = multi_block_xz_bytes(&tar);
        assert!(xz.len() > 128 * 1024);
        let source = XzSource::RawBytes(xz);

        let out = transcode(&source).await.unwrap();

        assert_eq!(gunzip(&out), tar);
        assert_eq!(uncompressed_size(&source).await, Some(tar.len() as u64));
    }

    #[tokio::test]
    async fn deflated_zip_member_is_also_decoded() {
        let tar = sample_tar();
        let zip = zip_bytes(
            kobo_root_xz_name(),
            &xz_bytes(&tar),
            zip::CompressionMethod::Deflated,
        );

        let out = transcode(&XzSource::ZipBytes(zip)).await.unwrap();

        assert_eq!(gunzip(&out), tar);
    }

    #[tokio::test]
    async fn corrupted_xz_payload_is_rejected() {
        let mut xz = xz_bytes(&sample_tar());
        let middle = xz.len() / 2;
        xz[middle] ^= 0xff;

        let error = transcode(&XzSource::RawBytes(xz)).await.unwrap_err();

        assert!(matches!(error, OtaError::Decompress(_)), "{error:?}");
    }

    #[tokio::test]
    async fn truncated_xz_payload_is_rejected() {
        let mut xz = xz_bytes(&sample_tar());
        xz.truncate(xz.len() - 20);

        let error = transcode(&XzSource::RawBytes(xz)).await.unwrap_err();

        assert!(matches!(error, OtaError::Decompress(_)), "{error:?}");
    }

    #[tokio::test]
    async fn zip_without_xz_member_is_a_deployment_error() {
        let zip = stored_zip("KoboRoot.tgz", b"gzip bundle");

        let error = transcode(&XzSource::ZipBytes(zip)).await.unwrap_err();

        assert!(matches!(error, OtaError::DeploymentError(_)), "{error:?}");
    }

    #[tokio::test]
    async fn cancellation_stops_between_chunks() {
        let source = XzSource::RawBytes(xz_bytes(&sample_tar()));
        let cancel = crate::http::CancelFlag::new();
        let mut out = Vec::new();

        let error = transcode_to_gzip(
            &source,
            &mut out,
            CancelFunc::from_flag(&cancel),
            &mut |_| cancel.request_cancel(),
        )
        .await
        .unwrap_err();

        assert!(matches!(error, OtaError::Cancelled), "{error:?}");
    }

    #[tokio::test]
    async fn zip_crc_mismatch_is_reported_even_when_xz_ends_first() {
        let xz = xz_bytes(&sample_tar());
        let mut zip = stored_zip(kobo_root_xz_name(), &xz);
        let local = zip
            .windows(4)
            .position(|window| window == b"PK\x03\x04")
            .unwrap();
        let central = zip
            .windows(4)
            .position(|window| window == b"PK\x01\x02")
            .unwrap();
        zip[local + 14] ^= 0xff;
        zip[central + 16] ^= 0xff;

        let error = transcode(&XzSource::ZipBytes(zip)).await.unwrap_err();

        assert!(
            matches!(&error, OtaError::ZipError(message) if message == "CRC32 checksum mismatch"),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn uncompressed_size_reads_the_xz_index_from_every_source() {
        let tar = sample_tar();
        let xz = xz_bytes(&tar);
        let zip = stored_zip(kobo_root_xz_name(), &xz);
        let dir = tempfile::tempdir().unwrap();
        let raw_path = dir.path().join("KoboRoot.tar.xz");
        std::fs::write(&raw_path, &xz).unwrap();
        let zip_path = dir.path().join("artifact.zip");
        std::fs::write(&zip_path, &zip).unwrap();

        for source in [
            XzSource::RawBytes(xz),
            XzSource::RawFile(raw_path),
            XzSource::ZipBytes(zip),
            XzSource::ZipFile(zip_path),
        ] {
            assert_eq!(
                uncompressed_size(&source).await,
                Some(tar.len() as u64),
                "{source:?}"
            );
        }
    }

    #[tokio::test]
    async fn uncompressed_size_is_unknown_for_a_deflated_zip_member() {
        let zip = zip_bytes(
            kobo_root_xz_name(),
            &xz_bytes(&sample_tar()),
            zip::CompressionMethod::Deflated,
        );

        assert_eq!(uncompressed_size(&XzSource::ZipBytes(zip)).await, None);
    }

    #[test]
    fn gzip_bound_covers_incompressible_input() {
        let noise: Vec<u8> = (0..2_000_000_u32)
            .map(|n| (n.wrapping_mul(2_654_435_761) >> 13) as u8 ^ (n >> 7) as u8)
            .collect();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&noise).unwrap();
        let gzip_len = encoder.finish().unwrap().len() as u64;

        assert!(gzip_len <= gzip_bound(noise.len() as u64));
        assert_eq!(gzip_bound(0), 31);
        assert_eq!(gzip_bound(u64::MAX), u64::MAX);
    }
}
