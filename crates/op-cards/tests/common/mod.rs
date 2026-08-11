//! Shared fixtures for this crate's integration tests.
//!
//! Each test binary compiles this module separately and uses a different subset
//! of it, so anything used by only one of them looks dead to the others.
#![allow(dead_code)]

use op_core::card::CardDb;

/// The fetched card data, or `None` when it has not been ingested.
///
/// The path is relative to this crate rather than absolute because a test
/// binary has no other fixed point: `CARGO_MANIFEST_DIR` is the only location
/// Cargo hands it, and the workspace `data/` sits two levels up from there.
/// Kept in one place so that stays a single fact rather than one per test file.
pub fn data_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")
}

/// Loads the card database, or `None` to skip — a bare clone has no `data/`,
/// and the suite stays green on one.
pub fn card_db() -> Option<CardDb> {
    CardDb::load_dir(data_dir().join("cards")).ok()
}
