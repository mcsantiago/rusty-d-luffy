//! Fetching card data on first run, and where it lives.
//!
//! The fetch itself is [`op_ingest`], in Rust. It used to shell out to
//! `tools/ingest/fetch_cards.py`, which was fine while this only ran from a
//! checkout but cannot ship: `python3` is absent by default on Windows, where
//! the name often resolves to a Store stub that opens the Store rather than
//! running anything.
//!
//! This module is the bridge to the UI — deciding what still needs fetching,
//! and turning progress into events.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// Event name the UI listens on for ingest progress.
pub const PROGRESS_EVENT: &str = "ingest://progress";

/// Bundle identifier, and the per-user data directory's name.
const APP_ID: &str = "dev.onepiecesim.desktop";

#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    /// A line of output, or empty on a pure counter update.
    pub line: String,
    pub done: bool,
    /// Meaningful only when `done`.
    pub ok: bool,
    /// Which pass is running: "packs" or "images".
    pub phase: Option<String>,
    pub current: u64,
    pub total: u64,
}

impl Progress {
    fn line(line: impl Into<String>) -> Progress {
        Progress {
            line: line.into(),
            done: false,
            ok: false,
            phase: None,
            current: 0,
            total: 0,
        }
    }

    fn counter(phase: &str, current: u64, total: u64) -> Progress {
        Progress {
            line: String::new(),
            done: false,
            ok: false,
            phase: Some(phase.to_string()),
            current,
            total,
        }
    }

    pub fn finished(ok: bool, line: impl Into<String>) -> Progress {
        Progress {
            line: line.into(),
            done: true,
            ok,
            phase: None,
            current: 0,
            total: 0,
        }
    }
}

pub use op_ingest::is_populated;

/// How many known cards have no cached art.
///
/// Card data being present does not mean the download finished — art is the
/// overwhelming majority of it, and an interrupted run leaves cards complete
/// and art partial. Checking only for card JSON would call that finished.
pub fn missing_art(db: &op_core::card::CardDb, images_dir: &Path) -> usize {
    db.iter()
        .filter(|(_, def)| def.number != "DON")
        .filter(|(_, def)| !images_dir.join(format!("{}.png", def.number)).exists())
        .count()
}

/// Where card data lives.
///
/// A checkout's `data/` wins when present, so development keeps using the
/// working copy. Otherwise the platform's per-user application data directory —
/// never the install directory, which is not user-writable on Windows or macOS.
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
    op_ingest::default_data_dir(APP_ID)
}

/// Everything, in one pass, the way a phone TCG client does it: one long
/// download on first launch, then the app is fully offline.
///
/// Resumable — anything already on disk is skipped — and reports per-file
/// counts, because a download this long with no feedback is indistinguishable
/// from a hang.
pub fn run(app: &AppHandle, data_dir: &Path) -> Result<(), String> {
    let plan = op_ingest::Plan {
        packs: Vec::new(), // every pack
        images: true,
        refresh: false,
        jobs: 4,
    };

    let report = |p: op_ingest::Progress| {
        let progress = match p {
            op_ingest::Progress::Message(line) => Progress::line(line),
            op_ingest::Progress::Counter {
                phase,
                current,
                total,
            } => Progress::counter(phase, current, total),
        };
        // A UI that has gone away is not a reason to abort the download.
        let _ = app.emit(PROGRESS_EVENT, progress);
    };

    match op_ingest::run(data_dir, &plan, &report) {
        Ok(summary) if summary.failed.is_empty() => {
            let _ = app.emit(
                PROGRESS_EVENT,
                Progress::finished(
                    true,
                    format!("{} cards ready", summary.cards),
                ),
            );
            Ok(())
        }
        Ok(summary) => {
            let message = format!("{} product(s) failed: {}", summary.failed.len(), summary.failed.join(", "));
            let _ = app.emit(PROGRESS_EVENT, Progress::finished(false, &message));
            Err(message)
        }
        Err(err) => {
            let message = err.to_string();
            let _ = app.emit(PROGRESS_EVENT, Progress::finished(false, &message));
            Err(message)
        }
    }
}
