//! Where card data lives, and making sure it is there.
//!
//! Shared by the client and by `op-replay` deliberately. A replay tool that
//! resolved card data differently from the client that wrote the log would
//! diverge for reasons that have nothing to do with the engine — which is the
//! one failure a replay is supposed to rule out.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use op_core::card::CardDb;

/// A checkout's `data/` wins when present; otherwise the per-user data
/// directory, shared with the desktop client so they do not fetch twice.
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("OPSIM_DATA_DIR") {
        return PathBuf::from(dir);
    }
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
    if checkout.join("cards").is_dir() {
        if let Ok(dir) = checkout.canonicalize() {
            return dir;
        }
    }
    op_ingest::default_data_dir("dev.onepiecesim.desktop")
}

/// Loads the card database, fetching it first if this is a bare checkout.
pub fn load_db(data_dir: &Path) -> Result<CardDb> {
    load_db_inner(data_dir, true)
}

/// Loads the card database, failing rather than fetching when it is absent.
///
/// For tools that verify rather than play: a mistyped `--data-dir` should be an
/// error, not a several-hundred-request download of the entire card pool.
pub fn load_db_offline(data_dir: &Path) -> Result<CardDb> {
    load_db_inner(data_dir, false)
}

fn load_db_inner(data_dir: &Path, may_fetch: bool) -> Result<CardDb> {
    let cards = data_dir.join("cards");
    if !op_ingest::is_populated(&cards) && !may_fetch {
        anyhow::bail!(
            "no card data in {} — fetch it first with `op-fetch --data-dir {}`",
            cards.display(),
            data_dir.display()
        );
    }
    if !op_ingest::is_populated(&cards) {
        println!("No card data yet — fetching (this happens once)...");
        let plan = op_ingest::Plan {
            packs: Vec::new(),
            images: false, // the terminal client draws text, not art
            refresh: false,
            jobs: 4,
        };
        op_ingest::run(data_dir, &plan, &|p| {
            if let op_ingest::Progress::Message(m) = p {
                println!("  {m}");
            }
        })
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    CardDb::load_dir(&cards).with_context(|| "loading card data")
}

/// Where session logs go, or `None` when disabled.
///
/// Matches the desktop client's rule so a bug report means the same thing
/// whichever client produced it: `OPSIM_DEBUG_DIR=` (empty) turns logging off,
/// anything else overrides the default of `<data>/debug`.
pub fn debug_dir(override_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = override_dir {
        return Some(dir.to_path_buf());
    }
    match std::env::var("OPSIM_DEBUG_DIR") {
        Ok(dir) if dir.is_empty() => None,
        Ok(dir) => Some(PathBuf::from(dir)),
        Err(_) => Some(data_dir().join("debug")),
    }
}
