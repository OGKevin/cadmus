//! Reads one page of matching lines from a log, in either direction.
//!
//! Cursors are byte offsets on line boundaries. A page covers a range of the
//! file, `start` to `end`, and stops early when the read budget runs out so a
//! filter with few matches reports how far it got instead of holding the
//! request. An unterminated last line is never consumed: it may still be
//! written, so a later page picks it up whole.

use super::filter::LineFilter;
use memchr::{memchr, memrchr};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

/// Input one page request may read.
pub(super) const SCAN_BUDGET: u64 = 16 * 1024 * 1024;

/// Most lines one page returns.
pub(super) const MAX_LIMIT: usize = 1000;

/// Lines per page when the request names none.
pub(super) const DEFAULT_LIMIT: usize = 200;

const BLOCK: usize = 64 * 1024;

/// Longest line text one page returns.
const MAX_TEXT: usize = 64 * 1024;

/// Bytes kept past [`MAX_TEXT`] to finish the character sequence at the cut.
const TEXT_SLACK: usize = 3;

/// Longest prefix of a line held in memory.
const TEXT_CAP: usize = MAX_TEXT + TEXT_SLACK;

/// Which way a page reads from its cursor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Direction {
    Forward,
    Backward,
}

/// Where a page starts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Anchor {
    /// The last complete line, reading backward.
    End,
    /// A line boundary, or where the previous partial page stopped.
    Cursor(u64),
}

pub(super) struct Request {
    pub(super) anchor: Anchor,
    pub(super) direction: Direction,
    pub(super) limit: usize,
    pub(super) filter: LineFilter,
}

/// A line and the offset it starts at.
pub(super) struct Line {
    pub(super) offset: u64,
    pub(super) text: Vec<u8>,
    pub(super) cut_from: u64,
}

pub(super) struct Page {
    pub(super) lines: Vec<Line>,
    pub(super) start: u64,
    pub(super) end: u64,
    pub(super) scanned_to: u64,
    pub(super) size: u64,
    pub(super) partial: bool,
}

/// Reads one page of `path`, scanning at most `budget` bytes.
///
/// # Errors
///
/// Returns the I/O error of the file, or `InvalidInput` when a forward cursor is
/// past the end of the file or does not follow a newline.
pub(super) fn read_page(path: &Path, request: &Request, budget: u64) -> io::Result<Page> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let limit = request.limit.clamp(1, MAX_LIMIT);
    match (request.anchor, request.direction) {
        (Anchor::End, _) => match last_boundary(&mut file, size, budget)? {
            Boundary::Line(cursor) => {
                backward(&mut file, size, cursor, limit, &request.filter, budget)
            }
            Boundary::Partial(scanned_to) => Ok(Page {
                lines: Vec::new(),
                start: scanned_to,
                end: size,
                scanned_to,
                size,
                partial: true,
            }),
        },
        (Anchor::Cursor(cursor), Direction::Backward) => {
            if cursor > size {
                return Err(invalid("cursor is past the end of the log"));
            }
            backward(&mut file, size, cursor, limit, &request.filter, budget)
        }
        (Anchor::Cursor(cursor), Direction::Forward) => {
            check_boundary(&mut file, size, cursor)?;
            forward(&mut file, size, cursor, limit, &request.filter, budget)
        }
    }
}

/// Where the last complete line of the file starts.
enum Boundary {
    /// Offset after the last newline, 0 when the file has none.
    Line(u64),
    /// Lowest offset scanned; no newline within the budget.
    Partial(u64),
}

/// Offset after the last newline of the file, 0 when it has none.
fn last_boundary(file: &mut File, size: u64, budget: u64) -> io::Result<Boundary> {
    let mut end = size;
    let mut scanned = 0u64;
    let mut block = vec![0u8; BLOCK];
    while end > 0 {
        if scanned > 0 && scanned >= budget {
            return Ok(Boundary::Partial(end));
        }
        let start = end.saturating_sub(BLOCK as u64);
        let len = (end - start) as usize;
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut block[..len])?;
        scanned += len as u64;
        if let Some(index) = memrchr(b'\n', &block[..len]) {
            return Ok(Boundary::Line(start + index as u64 + 1));
        }
        end = start;
    }
    Ok(Boundary::Line(0))
}

fn check_boundary(file: &mut File, size: u64, cursor: u64) -> io::Result<()> {
    if cursor > size {
        return Err(invalid("cursor is past the end of the log"));
    }
    if cursor == 0 {
        return Ok(());
    }
    if boundary_before(file, cursor)? {
        Ok(())
    } else {
        Err(invalid("cursor is not on a line boundary"))
    }
}

/// Whether the byte before `cursor` ends a line.
fn boundary_before(file: &mut File, cursor: u64) -> io::Result<bool> {
    if cursor == 0 {
        return Ok(true);
    }
    let mut previous = [0u8; 1];
    file.seek(SeekFrom::Start(cursor - 1))?;
    file.read_exact(&mut previous)?;
    Ok(previous[0] == b'\n')
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

/// Appends the start of `bytes` to `text`, keeping at most [`TEXT_CAP`] bytes.
fn append(text: &mut Vec<u8>, bytes: &[u8]) {
    let room = TEXT_CAP.saturating_sub(text.len());
    text.extend_from_slice(&bytes[..bytes.len().min(room)]);
}

/// Prepends `bytes` to `text`, keeping the earliest [`TEXT_CAP`] bytes.
fn prepend(text: &mut Vec<u8>, bytes: &[u8]) {
    if bytes.len() >= TEXT_CAP {
        text.clear();
        text.extend_from_slice(&bytes[..TEXT_CAP]);
        return;
    }
    let room = TEXT_CAP - bytes.len();
    if text.len() > room {
        text.truncate(room);
    }
    let held = text.len();
    text.reserve(bytes.len());
    text.resize(held + bytes.len(), 0);
    text.copy_within(0..held, bytes.len());
    text[..bytes.len()].copy_from_slice(bytes);
}

/// Shortens `text` to [`MAX_TEXT`] bytes on a character boundary.
///
/// Returns `full_len` when the line was longer, so the caller reports it as
/// `cut_from`; 0 when the text is the whole line.
fn fit_text(text: &mut Vec<u8>, full_len: u64) -> u64 {
    if full_len <= MAX_TEXT as u64 {
        return 0;
    }
    let mut end = MAX_TEXT;
    let floor = end.saturating_sub(TEXT_SLACK);
    while end > floor && (text[end] & 0xC0) == 0x80 {
        end -= 1;
    }
    text.truncate(end);
    full_len
}

/// Lines from `cursor` towards the end of the file.
fn forward(
    file: &mut File,
    size: u64,
    cursor: u64,
    limit: usize,
    filter: &LineFilter,
    budget: u64,
) -> io::Result<Page> {
    file.seek(SeekFrom::Start(cursor))?;
    let mut lines = Vec::new();
    let mut text: Vec<u8> = Vec::new();
    let mut line_len = 0u64;
    let mut line_offset = cursor;
    let mut consumed = cursor;
    let mut read_total = 0u64;
    let mut block = vec![0u8; BLOCK];
    let mut partial = false;
    'read: loop {
        if lines.len() >= limit {
            break;
        }
        if line_len == 0 && read_total > 0 && read_total >= budget {
            partial = true;
            break;
        }
        let read = file.read(&mut block)?;
        if read == 0 {
            break;
        }
        read_total += read as u64;
        let mut from = 0;
        while let Some(index) = memchr(b'\n', &block[from..read]) {
            let segment = &block[from..from + index];
            append(&mut text, segment);
            line_len += segment.len() as u64;
            consumed = line_offset + line_len + 1;
            if filter.matches(&text) {
                let cut_from = fit_text(&mut text, line_len);
                lines.push(Line {
                    offset: line_offset,
                    text: std::mem::take(&mut text),
                    cut_from,
                });
            }
            text.clear();
            line_len = 0;
            line_offset = consumed;
            from += index + 1;
            if lines.len() >= limit {
                break 'read;
            }
        }
        if from < read {
            let segment = &block[from..read];
            append(&mut text, segment);
            line_len += segment.len() as u64;
        }
    }
    Ok(Page {
        lines,
        start: cursor,
        end: consumed,
        scanned_to: consumed,
        size,
        partial,
    })
}

/// Lines from just before `cursor` towards the start of the file.
///
/// A cursor inside a line drops the bytes after that line's newline, so a
/// partial page can continue from where it stopped without showing half a line.
fn backward(
    file: &mut File,
    size: u64,
    cursor: u64,
    limit: usize,
    filter: &LineFilter,
    budget: u64,
) -> io::Result<Page> {
    let mut lines = Vec::new();
    let mut pending: Vec<u8> = Vec::new();
    let mut pending_len = 0u64;
    let mut discarding = !boundary_before(file, cursor)?;
    let mut consumed_start = cursor;
    let mut pos = if discarding {
        cursor
    } else {
        cursor.saturating_sub(1)
    };
    let mut read_total = 0u64;
    let mut block = vec![0u8; BLOCK];
    let mut partial = false;
    'read: while pos > 0 {
        if lines.len() >= limit {
            break;
        }
        if (pending_len == 0 || consumed_start != cursor) && read_total > 0 && read_total >= budget
        {
            partial = true;
            break;
        }
        let len = pos.min(BLOCK as u64) as usize;
        let block_start = pos - len as u64;
        file.seek(SeekFrom::Start(block_start))?;
        file.read_exact(&mut block[..len])?;
        read_total += len as u64;
        pos = block_start;
        let mut seg_end = len;
        loop {
            let Some(index) = memrchr(b'\n', &block[..seg_end]) else {
                if !discarding {
                    prepend(&mut pending, &block[..seg_end]);
                    pending_len += seg_end as u64;
                }
                break;
            };
            if discarding {
                discarding = false;
            } else {
                let segment = &block[index + 1..seg_end];
                prepend(&mut pending, segment);
                pending_len += segment.len() as u64;
                let offset = block_start + index as u64 + 1;
                if filter.matches(&pending) {
                    let cut_from = fit_text(&mut pending, pending_len);
                    lines.push(Line {
                        offset,
                        text: std::mem::take(&mut pending),
                        cut_from,
                    });
                }
                pending.clear();
                pending_len = 0;
                consumed_start = offset;
                if lines.len() >= limit {
                    break 'read;
                }
            }
            seg_end = index;
        }
    }
    if !partial && !discarding && cursor > 0 && lines.len() < limit {
        if filter.matches(&pending) {
            let cut_from = fit_text(&mut pending, pending_len);
            lines.push(Line {
                offset: 0,
                text: std::mem::take(&mut pending),
                cut_from,
            });
        }
        consumed_start = 0;
    }
    lines.reverse();
    let start = if lines.is_empty() && partial && consumed_start == cursor {
        pos
    } else {
        consumed_start
    };
    Ok(Page {
        lines,
        start,
        end: cursor,
        scanned_to: start,
        size,
        partial,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadmus_api_proto::cadmus::api::public::v1::LogFilter;
    use std::io::Write;

    fn log_of(lines: &[&str]) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
        file
    }

    fn request(anchor: Anchor, direction: Direction, limit: usize, text: &str) -> Request {
        Request {
            anchor,
            direction,
            limit,
            filter: LineFilter::new(Some(&LogFilter {
                text: text.to_owned(),
                ..Default::default()
            })),
        }
    }

    fn texts(page: &Page) -> Vec<String> {
        page.lines
            .iter()
            .map(|line| String::from_utf8(line.text.clone()).unwrap())
            .collect()
    }

    #[test]
    fn opens_at_the_end_with_the_newest_lines_in_file_order() {
        let file = log_of(&["a", "b", "c", "d"]);
        let page = read_page(
            file.path(),
            &request(Anchor::End, Direction::Backward, 2, ""),
            SCAN_BUDGET,
        )
        .unwrap();
        assert_eq!(texts(&page), ["c", "d"]);
        assert_eq!((page.start, page.end, page.size), (4, 8, 8));
        assert!(!page.partial);
    }

    #[test]
    fn paging_backward_reaches_the_first_line_then_stops() {
        let file = log_of(&["a", "b", "c", "d", "e"]);
        let first = read_page(
            file.path(),
            &request(Anchor::End, Direction::Backward, 2, ""),
            SCAN_BUDGET,
        )
        .unwrap();
        let second = read_page(
            file.path(),
            &request(Anchor::Cursor(first.start), Direction::Backward, 2, ""),
            SCAN_BUDGET,
        )
        .unwrap();
        let third = read_page(
            file.path(),
            &request(Anchor::Cursor(second.start), Direction::Backward, 2, ""),
            SCAN_BUDGET,
        )
        .unwrap();
        assert_eq!(texts(&first), ["d", "e"]);
        assert_eq!(texts(&second), ["b", "c"]);
        assert_eq!(texts(&third), ["a"]);
        assert_eq!(third.start, 0);
        let past = read_page(
            file.path(),
            &request(Anchor::Cursor(0), Direction::Backward, 2, ""),
            SCAN_BUDGET,
        )
        .unwrap();
        assert!(past.lines.is_empty());
        assert_eq!((past.start, past.end), (0, 0));
    }

    #[test]
    fn paging_forward_reaches_the_end_then_stops() {
        let file = log_of(&["a", "b", "c"]);
        let first = read_page(
            file.path(),
            &request(Anchor::Cursor(0), Direction::Forward, 2, ""),
            SCAN_BUDGET,
        )
        .unwrap();
        let second = read_page(
            file.path(),
            &request(Anchor::Cursor(first.end), Direction::Forward, 2, ""),
            SCAN_BUDGET,
        )
        .unwrap();
        let third = read_page(
            file.path(),
            &request(Anchor::Cursor(second.end), Direction::Forward, 2, ""),
            SCAN_BUDGET,
        )
        .unwrap();
        assert_eq!(texts(&first), ["a", "b"]);
        assert_eq!(texts(&second), ["c"]);
        assert!(third.lines.is_empty());
        assert_eq!(third.end, 6);
    }

    #[test]
    fn an_unterminated_last_line_waits_until_it_is_complete() {
        let mut file = log_of(&["a", "b"]);
        write!(file, "c").unwrap();
        file.flush().unwrap();
        let backward = read_page(
            file.path(),
            &request(Anchor::End, Direction::Backward, 5, ""),
            SCAN_BUDGET,
        )
        .unwrap();
        let forward = read_page(
            file.path(),
            &request(Anchor::Cursor(0), Direction::Forward, 5, ""),
            SCAN_BUDGET,
        )
        .unwrap();
        assert_eq!(texts(&backward), ["a", "b"]);
        assert_eq!(texts(&forward), ["a", "b"]);
        assert_eq!(forward.end, 4);
        writeln!(file).unwrap();
        let refreshed = read_page(
            file.path(),
            &request(Anchor::Cursor(forward.end), Direction::Forward, 5, ""),
            SCAN_BUDGET,
        )
        .unwrap();
        assert_eq!(texts(&refreshed), ["c"]);
    }

    #[test]
    fn a_filter_keeps_only_matching_lines_and_pages_stay_contiguous() {
        let file = log_of(&["wifi 1", "other", "wifi 2", "other", "wifi 3", "other"]);
        let first = read_page(
            file.path(),
            &request(Anchor::End, Direction::Backward, 2, "WIFI"),
            SCAN_BUDGET,
        )
        .unwrap();
        assert_eq!(texts(&first), ["wifi 2", "wifi 3"]);
        let second = read_page(
            file.path(),
            &request(Anchor::Cursor(first.start), Direction::Backward, 2, "WIFI"),
            SCAN_BUDGET,
        )
        .unwrap();
        assert_eq!(texts(&second), ["wifi 1"]);
        assert_eq!(second.start, 0);
    }

    #[test]
    fn budget_exhaustion_returns_a_partial_page_that_continues_where_it_stopped() {
        let filler = "x".repeat(1000);
        let mut rows = vec!["needle"];
        rows.extend(std::iter::repeat_n(filler.as_str(), 400));
        let file = log_of(&rows);
        let tiny = (BLOCK * 2) as u64;
        let mut cursor = file.as_file().metadata().unwrap().len();
        let mut found = Vec::new();
        let mut steps = 0;
        loop {
            let page = read_page(
                file.path(),
                &request(Anchor::Cursor(cursor), Direction::Backward, 5, "needle"),
                tiny,
            )
            .unwrap();
            found.extend(texts(&page));
            steps += 1;
            assert!(steps < 20, "page never finished");
            if !page.partial {
                break;
            }
            assert!(
                page.scanned_to < cursor,
                "a partial page must make progress"
            );
            cursor = page.scanned_to;
        }
        assert_eq!(found, ["needle"]);
        assert!(steps > 1, "the small budget should have split the scan");
    }

    #[test]
    fn lines_longer_than_the_limit_are_returned_cut_in_both_directions() {
        let long = "y".repeat(MAX_TEXT * 2 + 17);
        let file = log_of(&["a", &long, "b"]);
        for (anchor, direction) in [
            (Anchor::End, Direction::Backward),
            (Anchor::Cursor(0), Direction::Forward),
        ] {
            let page = read_page(
                file.path(),
                &request(anchor, direction, 10, ""),
                SCAN_BUDGET,
            )
            .unwrap();
            assert_eq!(page.lines.len(), 3);
            assert_eq!(page.lines[0].text.as_slice(), b"a");
            assert_eq!(page.lines[1].text.len(), MAX_TEXT);
            assert_eq!(page.lines[1].cut_from, long.len() as u64);
            assert_eq!(page.lines[2].text.as_slice(), b"b");
        }
    }

    #[test]
    fn a_cut_line_ends_on_a_character_boundary() {
        let long = "€".repeat(30_000);
        let file = log_of(&[&long]);
        for (anchor, direction) in [
            (Anchor::End, Direction::Backward),
            (Anchor::Cursor(0), Direction::Forward),
        ] {
            let page =
                read_page(file.path(), &request(anchor, direction, 5, ""), SCAN_BUDGET).unwrap();
            let line = &page.lines[0];
            assert_eq!(line.text.len(), MAX_TEXT - 1);
            assert_eq!(line.cut_from, long.len() as u64);
            assert!(std::str::from_utf8(&line.text).is_ok());
        }
    }

    #[test]
    fn a_line_longer_than_the_budget_still_progresses_forward() {
        let long = "y".repeat(BLOCK * 3 + 17);
        let file = log_of(&[&long]);
        let page = read_page(
            file.path(),
            &request(Anchor::Cursor(0), Direction::Forward, 10, ""),
            1024,
        )
        .unwrap();
        assert_eq!(page.lines.len(), 1);
        assert_eq!(page.lines[0].text.len(), MAX_TEXT);
        assert_eq!(page.lines[0].cut_from, long.len() as u64);
        assert!(page.partial);
        assert!(page.scanned_to > 0);
    }

    #[test]
    fn a_line_longer_than_the_budget_still_progresses_backward() {
        let long = "y".repeat(BLOCK * 3 + 17);
        let file = log_of(&["a", &long]);
        let page = read_page(
            file.path(),
            &request(Anchor::End, Direction::Backward, 10, ""),
            1024,
        )
        .unwrap();
        assert_eq!(page.lines.len(), 2);
        assert_eq!(page.lines[0].text.as_slice(), b"a");
        assert_eq!(page.lines[1].cut_from, long.len() as u64);
        assert_eq!(page.start, 0);
    }

    #[test]
    fn the_from_end_search_stops_at_the_budget_and_still_moves() {
        let long = "y".repeat(BLOCK * 3);
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "a").unwrap();
        write!(file, "{long}").unwrap();
        file.flush().unwrap();
        let page = read_page(
            file.path(),
            &request(Anchor::End, Direction::Backward, 10, ""),
            1,
        )
        .unwrap();
        assert!(page.lines.is_empty());
        assert!(page.partial);
        assert!(page.scanned_to < page.size);
        assert_eq!(page.start, page.scanned_to);
        let next = read_page(
            file.path(),
            &request(Anchor::Cursor(page.scanned_to), Direction::Backward, 10, ""),
            1,
        )
        .unwrap();
        assert!(next.scanned_to < page.scanned_to || !next.lines.is_empty());
    }

    #[test]
    fn line_offsets_name_where_each_line_starts() {
        let file = log_of(&["ab", "cde", "f"]);
        let page = read_page(
            file.path(),
            &request(Anchor::End, Direction::Backward, 10, ""),
            SCAN_BUDGET,
        )
        .unwrap();
        let offsets: Vec<u64> = page.lines.iter().map(|line| line.offset).collect();
        assert_eq!(offsets, [0, 3, 7]);
    }

    #[test]
    fn a_cursor_inside_a_line_or_past_the_end_is_invalid() {
        let file = log_of(&["abc", "def"]);
        for cursor in [2, 99] {
            let error = read_page(
                file.path(),
                &request(Anchor::Cursor(cursor), Direction::Forward, 5, ""),
                SCAN_BUDGET,
            )
            .err()
            .unwrap();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{cursor}");
        }
    }

    #[test]
    fn an_empty_file_has_no_lines() {
        let file = log_of(&[]);
        let page = read_page(
            file.path(),
            &request(Anchor::End, Direction::Backward, 5, ""),
            SCAN_BUDGET,
        )
        .unwrap();
        assert!(page.lines.is_empty());
        assert_eq!((page.start, page.end, page.size), (0, 0, 0));
    }
}
