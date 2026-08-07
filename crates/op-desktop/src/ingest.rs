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

/// Prefix the ingest script uses for machine-readable progress lines.
const PROGRESS_PREFIX: &str = "@progress";

#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    /// A line of output from the script, or a status message from here.
    /// Empty on a pure counter update, which the UI should not log.
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

    fn finished(ok: bool, line: impl Into<String>) -> Progress {
        Progress {
            line: line.into(),
            done: true,
            ok,
            phase: None,
            current: 0,
            total: 0,
        }
    }

    /// Parses `@progress <phase> <done> <total>`, or `None` for ordinary
    /// output.
    fn parse_counter(line: &str) -> Option<Progress> {
        let rest = line.strip_prefix(PROGRESS_PREFIX)?;
        let mut parts = rest.split_whitespace();
        let phase = parts.next()?.to_string();
        let current = parts.next()?.parse().ok()?;
        let total = parts.next()?.parse().ok()?;
        Some(Progress {
            line: String::new(),
            done: false,
            ok: false,
            phase: Some(phase),
            current,
            total,
        })
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

/// How many known cards have no cached art.
///
/// Card data being present does not mean the download finished — art is the
/// overwhelming majority of it, and an interrupted run leaves cards complete
/// and art partial. Checking only for card JSON would declare that state
/// finished and never fill the gap.
pub fn missing_art(db: &op_core::card::CardDb, images_dir: &Path) -> usize {
    db.iter()
        .filter(|(_, def)| def.number != "DON")
        .filter(|(_, def)| !images_dir.join(format!("{}.png", def.number)).exists())
        .count()
}

/// Everything, in one pass, the way a phone TCG client does it: one long
/// download on first launch, then the app is fully offline forever after.
///
/// That is ~2,700 images and roughly 736 MB, so it takes minutes rather than
/// seconds. Two things make it tolerable. The script skips files already on
/// disk, so an interrupted or killed run resumes rather than restarting. And it
/// reports per-file counts, so the UI can show a real bar — a download this
/// long with no feedback is indistinguishable from a hang.
const STEPS: &[(&str, &[&str])] = &[(
    "Downloading every set — card data and art…",
    &["--all", "--images"],
)];

/// Runs the ingest, emitting each line of output as it arrives.
///
/// Blocking — call it on a worker thread.
pub fn run(app: &AppHandle, repo_root: &Path) -> Result<(), String> {
    let script = repo_root.join("tools/ingest/fetch_cards.py");
    if !script.exists() {
        let message = format!("ingest script not found at {}", script.display());
        emit(app, Progress::finished(false, &message));
        return Err(message);
    }

    for (label, args) in STEPS {
        emit(app, Progress::line(*label));
        run_step(app, repo_root, &script, args)?;
    }

    emit(app, Progress::finished(true, "Card data ready"));
    Ok(())
}

fn run_step(
    app: &AppHandle,
    repo_root: &Path,
    script: &Path,
    args: &[&str],
) -> Result<(), String> {
    let mut child = match Command::new("python3")
        .arg(script)
        .args(args)
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
            if line.is_empty() {
                continue;
            }
            // Counter updates drive the bar; everything else is log output.
            match Progress::parse_counter(&line) {
                Some(counter) => emit(app, counter),
                None => emit(app, Progress::line(line)),
            }
        }
    }

    let status = child.wait().map_err(|e| e.to_string())?;
    if status.success() {
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
