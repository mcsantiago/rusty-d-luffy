//! Fetching card data on first run.
//!
//! The app is useless without `data/`, and requiring a manual Python step
//! before the first launch is a poor greeting. Startup therefore shells out to
//! `tools/ingest/fetch_cards.py` and streams its output to the UI.
//!
//! Shelling out rather than reimplementing the fetch keeps one copy of the
//! awkward parts — pack-name aliasing, alternate-printing filtering, retries —
//! which are easy to get subtly wrong twice. The cost is a dependency on
//! `python3` being present, which is reported plainly rather than swallowed.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// Event name the UI listens on for ingest progress.
pub const PROGRESS_EVENT: &str = "ingest://progress";

#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    /// A line of output from the script, or a status message from here.
    pub line: String,
    pub done: bool,
    /// Meaningful only when `done`.
    pub ok: bool,
}

impl Progress {
    fn line(line: impl Into<String>) -> Progress {
        Progress {
            line: line.into(),
            done: false,
            ok: false,
        }
    }

    fn finished(ok: bool, line: impl Into<String>) -> Progress {
        Progress {
            line: line.into(),
            done: true,
            ok,
        }
    }
}

/// Whether `data/cards` already holds something loadable.
pub fn is_populated(cards_dir: &Path) -> bool {
    std::fs::read_dir(cards_dir)
        .map(|mut entries| {
            entries.any(|e| {
                e.map(|e| e.path().extension().is_some_and(|x| x == "json"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// Runs the ingest script, emitting each line of output as it arrives.
///
/// Blocking — call it on a worker thread. Only the starter decks are fetched,
/// with their art: that is what the app can actually play, and it keeps first
/// run to a few dozen small requests rather than the full pool's ~2,700 files
/// and several hundred megabytes of images.
pub fn run(app: &AppHandle, repo_root: &Path) -> Result<(), String> {
    let script = repo_root.join("tools/ingest/fetch_cards.py");
    if !script.exists() {
        let message = format!("ingest script not found at {}", script.display());
        emit(app, Progress::finished(false, &message));
        return Err(message);
    }

    emit(app, Progress::line("Fetching card data…"));

    let mut child = match Command::new("python3")
        .arg(&script)
        .args(["--packs", "ST-01", "ST-02", "--images"])
        .current_dir(repo_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => {
            let message = format!(
                "could not run python3 ({err}).\n\
                 Fetch the data manually:\n  \
                 python3 tools/ingest/fetch_cards.py --images"
            );
            emit(app, Progress::finished(false, &message));
            return Err(message);
        }
    };

    // Stream stdout so the modal shows progress rather than sitting blank for
    // the whole download.
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let line = line.trim_end().to_string();
            if !line.is_empty() {
                emit(app, Progress::line(line));
            }
        }
    }

    let status = child.wait().map_err(|e| e.to_string())?;
    if status.success() {
        emit(app, Progress::finished(true, "Card data ready"));
        Ok(())
    } else {
        // stderr is drained only on failure; on success it is just retry noise.
        let mut detail = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            use std::io::Read;
            let _ = stderr.read_to_string(&mut detail);
        }
        let message = format!(
            "ingest failed ({status}). {}",
            detail.lines().last().unwrap_or("")
        );
        emit(app, Progress::finished(false, &message));
        Err(message)
    }
}

fn emit(app: &AppHandle, progress: Progress) {
    // A UI that has gone away is not a reason to abort the download.
    let _ = app.emit(PROGRESS_EVENT, progress);
}

/// The repository root, resolved from this crate so the app runs from a
/// checkout without an install step.
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
}
