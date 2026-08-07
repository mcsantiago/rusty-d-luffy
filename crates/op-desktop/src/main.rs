//! Desktop client.
//!
//! Tauri rather than Electron because the engine is already Rust: it links into
//! this binary and a UI click calls `Game::step` directly, with no sidecar
//! process and no hand-written IPC protocol. The front end is plain ES modules
//! under `client/`, so there is no JavaScript build step either.
//!
//! The window opens before card data is loaded. On a fresh checkout `data/` is
//! empty — it is Bandai's copyright and not vendored — so startup fetches it
//! behind a progress modal rather than refusing to launch.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ingest;
mod render;
mod session;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use base64::Engine;
use op_cards::Cards;
use op_core::card::CardDb;
use op_core::script::ScriptSource;
use op_core::DeckList;
use serde::Serialize;
use session::{CardInfo, Difficulty, Session, Snapshot};

/// Built-in decks, so a game can be started without a deckbuilder.
fn deck(leader: &str, spec: &[(&str, usize)]) -> DeckList {
    let mut cards = Vec::new();
    for (number, n) in spec {
        for _ in 0..*n {
            cards.push(number.to_string());
        }
    }
    DeckList {
        leader: leader.into(),
        cards,
    }
}

fn st01() -> DeckList {
    deck("ST01-001", &[
        ("ST01-002", 4), ("ST01-003", 4), ("ST01-004", 4), ("ST01-005", 2),
        ("ST01-006", 4), ("ST01-007", 4), ("ST01-008", 2), ("ST01-009", 4),
        ("ST01-010", 2), ("ST01-011", 4), ("ST01-012", 2), ("ST01-013", 4),
        ("ST01-014", 4), ("ST01-015", 2), ("ST01-016", 2), ("ST01-017", 2),
    ])
}

fn st02() -> DeckList {
    deck("ST02-001", &[
        ("ST02-002", 4), ("ST02-003", 4), ("ST02-004", 4), ("ST02-005", 4),
        ("ST02-006", 2), ("ST02-007", 4), ("ST02-008", 4), ("ST02-009", 2),
        ("ST02-010", 2), ("ST02-011", 4), ("ST02-012", 4), ("ST02-013", 2),
        ("ST02-014", 2), ("ST02-015", 4), ("ST02-016", 2), ("ST02-017", 2),
    ])
}

fn deck_by_name(name: &str) -> DeckList {
    match name {
        "ST02" => st02(),
        _ => st01(),
    }
}

/// Card data, once it has been fetched and loaded.
struct Loaded {
    db: Arc<CardDb>,
    scripts: Arc<dyn ScriptSource + Send + Sync>,
}

struct AppState {
    repo_root: std::path::PathBuf,
    data_dir: std::path::PathBuf,
    /// `None` until card data has been fetched and parsed.
    cards: RwLock<Option<Loaded>>,
    /// Set while an ingest is in flight, so a second bootstrap call from a
    /// reloaded window does not start a duplicate download.
    ingesting: Mutex<bool>,
    session: Mutex<Option<Session>>,
    /// Card art, base64-encoded on first request and kept for the process.
    art: Mutex<HashMap<String, Option<String>>>,
}

impl AppState {
    fn cards_dir(&self) -> std::path::PathBuf {
        self.data_dir.join("cards")
    }

    /// Parses `data/cards` into the shared database.
    fn load_cards(&self) -> Result<usize, String> {
        let db = CardDb::load_dir(self.cards_dir()).map_err(|e| e.to_string())?;
        let count = db.len();
        let scripts: Arc<dyn ScriptSource + Send + Sync> = Arc::new(Cards::new(&db));
        *self.cards.write().unwrap() = Some(Loaded {
            db: Arc::new(db),
            scripts,
        });
        Ok(count)
    }
}

#[derive(Serialize)]
struct BootstrapStatus {
    /// Card data is loaded and a game can be started.
    ready: bool,
    /// A fetch is running; the UI should watch `ingest://progress`.
    fetching: bool,
    message: String,
}

/// Called by the UI as soon as the window is up.
///
/// Loads card data if it is already on disk, and otherwise kicks off a fetch on
/// a worker thread so the window stays responsive.
#[tauri::command]
fn bootstrap(app: tauri::AppHandle, state: tauri::State<'_, AppState>) -> BootstrapStatus {
    if state.cards.read().unwrap().is_some() {
        return BootstrapStatus {
            ready: true,
            fetching: false,
            message: "Card data ready".into(),
        };
    }

    // Load whatever is already on disk first, so a complete install starts
    // instantly and offline.
    let mut ready = false;
    let mut message = String::new();
    if ingest::is_populated(&state.cards_dir()) {
        match state.load_cards() {
            Ok(count) => {
                ready = true;
                message = format!("{count} cards loaded");
            }
            Err(err) => message = err,
        }
    }

    // Card data present does not mean the download finished: art is the bulk of
    // it, and an interrupted run leaves cards complete and art partial. Ask what
    // is actually missing rather than assuming.
    let outstanding = if ready {
        let guard = state.cards.read().unwrap();
        let db = &guard.as_ref().expect("just loaded").db;
        ingest::missing_art(db, &state.data_dir.join("images"))
    } else {
        usize::MAX
    };

    if ready && outstanding == 0 {
        return BootstrapStatus {
            ready: true,
            fetching: false,
            message,
        };
    }

    // Guard against a reloaded window starting a second download.
    {
        let mut ingesting = state.ingesting.lock().unwrap();
        if *ingesting {
            return BootstrapStatus {
                ready,
                fetching: true,
                message: "Downloading card data…".into(),
            };
        }
        *ingesting = true;
    }

    let repo_root = state.repo_root.clone();
    std::thread::spawn(move || {
        let result = ingest::run(&app, &repo_root);
        let state = tauri::Manager::state::<AppState>(&app);
        if result.is_ok() {
            // Parsing happens here so the UI's "ready" signal means genuinely
            // ready, not merely downloaded.
            if let Err(err) = state.load_cards() {
                let _ = tauri::Emitter::emit(
                    &app,
                    ingest::PROGRESS_EVENT,
                    ingest::Progress {
                        line: format!("card data downloaded but failed to load: {err}"),
                        done: true,
                        ok: false,
                        phase: None,
                        current: 0,
                        total: 0,
                    },
                );
            }
        }
        *state.ingesting.lock().unwrap() = false;
    });

    BootstrapStatus {
        ready,
        fetching: true,
        message: if ready {
            // Playable already; the rest of the art tops up in the background.
            format!("{outstanding} card images still to download")
        } else {
            "Downloading card data…".into()
        },
    }
}

#[derive(Serialize)]
struct StartResult {
    snapshot: Snapshot,
    catalogue: Vec<CardInfo>,
}

#[tauri::command]
fn new_game(
    state: tauri::State<'_, AppState>,
    seed: Option<u64>,
    your_deck: String,
    ai_deck: String,
    difficulty: String,
    you_first: bool,
) -> Result<StartResult, String> {
    let guard = state.cards.read().unwrap();
    let loaded = guard.as_ref().ok_or("card data is not loaded yet")?;

    let seed = seed.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    });

    let human = deck_by_name(&your_deck);
    let ai = deck_by_name(&ai_deck);

    let session = Session::new(
        Arc::clone(&loaded.db),
        Arc::clone(&loaded.scripts),
        seed,
        human.clone(),
        ai.clone(),
        you_first,
        Difficulty::parse(&difficulty),
    )
    .map_err(|e| e.to_string())?;

    let catalogue = session.catalogue(&[&human, &ai]);
    let snapshot = session.snapshot();
    *state.session.lock().unwrap() = Some(session);

    Ok(StartResult {
        snapshot,
        catalogue,
    })
}

#[tauri::command]
fn choose(state: tauri::State<'_, AppState>, index: usize) -> Result<Snapshot, String> {
    let mut guard = state.session.lock().unwrap();
    let session = guard.as_mut().ok_or("no game in progress")?;
    session.choose(index)?;
    Ok(session.snapshot())
}

#[tauri::command]
fn snapshot(state: tauri::State<'_, AppState>) -> Result<Snapshot, String> {
    let guard = state.session.lock().unwrap();
    let session = guard.as_ref().ok_or("no game in progress")?;
    Ok(session.snapshot())
}

/// Card art as a data URI, or `None` when it has not been cached.
///
/// Served through a command rather than the asset protocol so the app degrades
/// gracefully: without `data/images` the UI falls back to drawing text cards.
#[tauri::command]
fn card_art(state: tauri::State<'_, AppState>, number: String) -> Option<String> {
    if let Some(cached) = state.art.lock().unwrap().get(&number) {
        return cached.clone();
    }
    let path = state.data_dir.join("images").join(format!("{number}.png"));
    let encoded = std::fs::read(&path).ok().map(|bytes| {
        format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        )
    });
    state.art.lock().unwrap().insert(number, encoded.clone());
    encoded
}

fn main() {
    let repo_root = ingest::repo_root();
    let data_dir = repo_root.join("data");

    // Card data is deliberately *not* loaded here. It may not exist yet, and a
    // window that opens and explains itself beats a process that exits.
    tauri::Builder::default()
        .manage(AppState {
            repo_root,
            data_dir,
            cards: RwLock::new(None),
            ingesting: Mutex::new(false),
            session: Mutex::new(None),
            art: Mutex::new(HashMap::new()),
        })
        .invoke_handler(tauri::generate_handler![
            bootstrap, new_game, choose, snapshot, card_art
        ])
        .run(tauri::generate_context!())
        .expect("failed to start the desktop app");
}
