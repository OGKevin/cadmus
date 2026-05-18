//! Shared types for dictionary index readers.

use super::Metadata;

#[derive(Debug, Clone)]
pub struct Entry {
    pub headword: String,
    pub offset: u64,
    pub size: u64,
    pub original: Option<String>,
}

pub trait IndexReader {
    fn load_and_find(&mut self, headword: &str, fuzzy: bool, metadata: &Metadata) -> Vec<Entry>;
    fn find(&self, headword: &str, fuzzy: bool) -> Vec<Entry>;
}
