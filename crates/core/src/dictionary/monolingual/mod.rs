//! Monolingual dictionary support.
//!
//! The single entry point for callers is [`MonolingualDictionaryService`],
//! which exposes:
//!
//! - [`MonolingualDictionaryService::get_available_dictionaries`] – remote catalogue
//! - [`MonolingualDictionaryService::get_installed_dictionaries`] – locally installed
//! - [`MonolingualDictionaryService::install_dictionary`] – download and extract
//!
//! All internal modules (`client`, `db`, `metadata`) are private implementation
//! details and are not accessible outside this module.

mod client;
mod db;
mod errors;
mod metadata;
mod service;

pub(crate) use service::MonolingualDictionaryService;
