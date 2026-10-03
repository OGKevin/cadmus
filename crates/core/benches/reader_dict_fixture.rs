//! Download Reader-Dict English `.index` and load it into an in-memory SQLite DB.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use cadmus_core::db::Database;
use cadmus_core::dictionary::Metadata;
use cadmus_core::dictionary::db_index::DbIndexReader;
use cadmus_core::dictionary::indexing::{Entry, IndexReader, normalize};
use cadmus_core::http::Client;
use sqlx::SqlitePool;
use tokio::io::{AsyncBufReadExt, BufReader as AsyncBufReader};
use zip::ZipArchive;

const READER_DICT_ZIP_URL: &str = "https://www.reader-dict.com/file/en/dictorg-en-en-noetym.zip";
const INDEX_FILE_NAMES: &[&str] = &["Reader-Dict-en.index", "dictorg-en-en.index"];
const READER_DICT_BENCH_DICT_ID: i64 = 1;
const BATCH_SIZE: usize = 5000;

pub(crate) struct ReaderDictFixture {
    pub db: Database,
    pub entry_count: usize,
    pub headword: String,
}

static READER_DICT: OnceLock<ReaderDictFixture> = OnceLock::new();

pub(crate) async fn init_reader_dict_fixture(headword: &str) -> &'static ReaderDictFixture {
    if let Some(fixture) = READER_DICT.get() {
        return fixture;
    }

    let index_path = ensure_reader_dict_index_on_disk().await;
    let (db, entry_count) = index_reader_dict_into_memory(&index_path).await;

    let scoped_hits = DbIndexReader::new(&db, Some(READER_DICT_BENCH_DICT_ID))
        .find(headword, true)
        .await
        .len();
    assert!(
        scoped_hits > 0,
        "headword must match Reader-Dict-en via DbIndexReader"
    );

    let _ = READER_DICT.set(ReaderDictFixture {
        db,
        entry_count,
        headword: headword.to_string(),
    });
    READER_DICT
        .get()
        .expect("reader dict fixture just initialized")
}

fn cache_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CADMUS_DICT_BENCH_CACHE") {
        return PathBuf::from(dir);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/dict-bench-cache")
}

async fn ensure_reader_dict_index_on_disk() -> PathBuf {
    if let Ok(path) = std::env::var("CADMUS_DICT_BENCH_INDEX") {
        let path = PathBuf::from(path);
        assert!(
            path.is_file(),
            "CADMUS_DICT_BENCH_INDEX must point at an existing .index file"
        );
        return path;
    }

    let cache = cache_dir();
    std::fs::create_dir_all(&cache).expect("create dict bench cache dir");

    for name in INDEX_FILE_NAMES {
        let cached = cache.join(name);
        if cached.is_file() {
            return cached;
        }
    }

    let zip_path = cache.join("dictorg-en-en-noetym.zip");
    if !zip_path.is_file() {
        eprintln!("dictionary_bench: downloading Reader-Dict-en from {READER_DICT_ZIP_URL}");
        let client = Client::new().expect("HTTP client for dictionary download");
        let response = client
            .get(READER_DICT_ZIP_URL)
            .send()
            .await
            .expect("download Reader-Dict-en archive");
        let bytes = response.bytes().await.expect("read Reader-Dict-en archive");
        std::fs::write(&zip_path, bytes).expect("write Reader-Dict-en zip to cache");
    }

    let index_path = extract_index_from_zip(&zip_path, &cache);
    eprintln!(
        "dictionary_bench: using Reader-Dict-en index at {}",
        index_path.display()
    );
    index_path
}

fn extract_index_from_zip(zip_path: &Path, dest_dir: &Path) -> PathBuf {
    for name in INDEX_FILE_NAMES {
        let dest = dest_dir.join(name);
        if dest.is_file() {
            return dest;
        }
    }

    let file = std::fs::File::open(zip_path).expect("open Reader-Dict-en zip");
    let mut archive = ZipArchive::new(file).expect("read Reader-Dict-en zip");

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).expect("zip entry");
        let entry_name = entry.name().to_string();
        let file_name = entry_name.rsplit('/').next().unwrap_or(entry_name.as_str());
        if !file_name.ends_with(".index") {
            continue;
        }

        let dest = dest_dir.join(file_name);
        let mut out = std::fs::File::create(&dest).expect("create extracted index file");
        std::io::copy(&mut entry, &mut out).expect("extract index from zip");
        return dest;
    }

    panic!("no .index member found in Reader-Dict-en zip");
}

fn decode_number(word: &str) -> Option<u64> {
    let mut index = 0u64;
    for (i, ch) in word.chars().rev().enumerate() {
        let base: u64 = match ch {
            'A'..='Z' => (ch as u64) - 65,
            'a'..='z' => (ch as u64) - 71,
            '0'..='9' => (ch as u64) + 4,
            '+' => 62,
            '/' => 63,
            _ => return None,
        };
        index += base * 64u64.pow(i as u32);
    }
    Some(index)
}

fn detect_metadata(path: &Path) -> Metadata {
    let file = std::fs::File::open(path).expect("open index for metadata detection");
    let reader = BufReader::new(file);

    let mut all_chars = false;
    let mut case_sensitive = false;

    for line in reader.lines().map_while(Result::ok) {
        let word = line.split('\t').next().unwrap_or("");
        match word {
            "00-database-allchars" => all_chars = true,
            "00-database-case-sensitive" => case_sensitive = true,
            _ => {}
        }
        if word.starts_with("00-database-") && word > "00-database-case-sensitive" {
            break;
        }
    }

    Metadata {
        all_chars,
        case_sensitive,
    }
}

fn parse_index_line(line: &str) -> Option<(String, i64, i64)> {
    let trimmed = line.trim_end();
    let mut cols = trimmed.split('\t');
    let word = cols.next()?;
    let offset = decode_number(cols.next()?)? as i64;
    let size = decode_number(cols.next()?)? as i64;
    Some((word.to_string(), offset, size))
}

async fn index_reader_dict_into_memory(index_path: &Path) -> (Database, usize) {
    let metadata = detect_metadata(index_path);
    let mut db = Database::new(":memory:")
        .await
        .expect("in-memory database for Reader-Dict-en bench");
    db.init_for_test(0).await.expect("run migrations");

    let pool = db.pool().clone();
    sqlx::query(
        "INSERT INTO dictionary_index_meta (dict_id, fingerprint, dict_path, total_lines, indexed_lines, completed) VALUES (?, ?, ?, 0, 0, 0)",
    )
    .bind(READER_DICT_BENCH_DICT_ID)
    .bind("reader-dict-en-bench")
    .bind(index_path.display().to_string())
    .execute(&pool)
    .await
    .expect("insert dictionary_index_meta");

    let file = tokio::fs::File::open(index_path)
        .await
        .expect("open Reader-Dict-en index");
    let mut lines = AsyncBufReader::new(file).lines();

    let mut raw_batch: Vec<Entry> = Vec::with_capacity(BATCH_SIZE);
    let mut entry_count = 0usize;

    while let Some(line) = lines.next_line().await.expect("read index line") {
        if let Some((word, offset, size)) = parse_index_line(&line) {
            if word.starts_with("00-database-") {
                continue;
            }
            raw_batch.push(Entry {
                headword: word,
                offset: offset as u64,
                size: size as u64,
                original: None,
            });
        }

        if raw_batch.len() >= BATCH_SIZE {
            entry_count += flush_normalized_batch(&pool, &raw_batch, &metadata).await;
            raw_batch.clear();
        }
    }

    if !raw_batch.is_empty() {
        entry_count += flush_normalized_batch(&pool, &raw_batch, &metadata).await;
    }

    sqlx::query(
        "UPDATE dictionary_index_meta SET completed = 1, indexed_lines = ? WHERE dict_id = ?",
    )
    .bind(entry_count as i64)
    .bind(READER_DICT_BENCH_DICT_ID)
    .execute(&pool)
    .await
    .expect("mark dictionary index complete");

    eprintln!(
        "dictionary_bench: indexed {entry_count} Reader-Dict-en headwords into :memory: SQLite"
    );

    (db, entry_count)
}

async fn flush_normalized_batch(
    pool: &SqlitePool,
    raw_batch: &[Entry],
    metadata: &Metadata,
) -> usize {
    let normalized = normalize(raw_batch, metadata);
    let inserted = normalized.len();
    let mut tx = pool.begin().await.expect("begin index batch tx");
    for entry in &normalized {
        sqlx::query(
            "INSERT OR IGNORE INTO dictionary_index_entry (dict_id, word, offset, size, original) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(READER_DICT_BENCH_DICT_ID)
        .bind(&entry.headword)
        .bind(entry.offset as i64)
        .bind(entry.size as i64)
        .bind(entry.original.as_deref())
        .execute(&mut *tx)
        .await
        .expect("insert dictionary_index_entry");
    }
    tx.commit().await.expect("commit index batch");
    inserted
}

pub(crate) fn default_reader_dict_headword() -> String {
    std::env::var("CADMUS_DICT_BENCH_HEADWORD").unwrap_or_else(|_| "abandon".to_string())
}
