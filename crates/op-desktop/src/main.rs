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

mod decks;
mod ingest;
mod render;
mod session;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use base64::Engine;
use op_cards::Cards;
use op_core::card::CardDb;
use op_core::script::ScriptSource;
use op_deck::store::DeckStore;
use serde::Serialize;
use session::{CardInfo, Difficulty, Session, Snapshot};

/// The saved-deck directory, opened per call.
///
/// Cheap — it creates the directory and keeps a path — and re-reading each time
/// means a deck imported in another window shows up without a restart.
fn deck_store() -> Result<DeckStore, String> {
    DeckStore::open(ingest::decks_dir()).map_err(|e| e.to_string())
}

/// The decks the client may offer.
///
/// Served rather than hardcoded in `index.html`, so a newly scripted set
/// reaches the menu by being added to `op_cards::decks::ALL` and nowhere else.
/// The previous copy in the markup is why ST-04 and ST-08 were fully scripted
/// and playable while the picker still showed three decks.
#[tauri::command]
fn decks(state: tauri::State<'_, AppState>) -> Vec<decks::DeckChoice> {
    let Ok(store) = deck_store() else {
        return Vec::new();
    };
    let guard = state.cards.read().unwrap();
    decks::choices(&store, guard.as_ref().map(Loaded::as_deck_data))
}

/// Card data, once it has been fetched and loaded.
///
/// Holds the concrete [`Cards`] rather than a `dyn ScriptSource`: the engine
/// wants the trait object, but deck compatibility wants `CardSupport`, and one
/// concrete value serves both.
struct Loaded {
    db: Arc<CardDb>,
    cards: Arc<Cards>,
}

impl Loaded {
    fn scripts(&self) -> Arc<dyn ScriptSource + Send + Sync> {
        Arc::clone(&self.cards) as Arc<dyn ScriptSource + Send + Sync>
    }

    fn as_deck_data(&self) -> decks::Loaded<'_> {
        decks::Loaded {
            db: &self.db,
            cards: &self.cards,
        }
    }
}

struct AppState {
    data_dir: std::path::PathBuf,
    /// `None` until card data has been fetched and parsed.
    cards: RwLock<Option<Loaded>>,
    /// Set while an ingest is in flight, so a second bootstrap call from a
    /// reloaded window does not start a duplicate download.
    ingesting: Mutex<bool>,
    /// Set while the AI is computing a turn, so overlapping requests do not
    /// stack up workers all contending for the session lock.
    ai_thinking: Mutex<bool>,
    /// Set once an ingest has run to completion this session.
    ///
    /// Needed because a handful of cards may have no art upstream at all. Left
    /// purely to a missing-art count, those would gate the UI forever; one
    /// successful full pass means the install is as complete as it can be.
    install_complete: Mutex<bool>,
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
        let cards = Arc::new(Cards::new(&db));
        *self.cards.write().unwrap() = Some(Loaded {
            db: Arc::new(db),
            cards,
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

    // "Ready" means the whole install is done, not merely that cards parsed.
    // Starting a game with art still arriving would show text placeholders,
    // because images download across all 59 sets in no particular order.
    let complete = *state.install_complete.lock().unwrap();
    if ready && (outstanding == 0 || complete) {
        return BootstrapStatus {
            ready: true,
            fetching: false,
            message,
        };
    }
    ready = false;

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

    let data_dir = state.data_dir.clone();
    std::thread::spawn(move || {
        let result = ingest::run(&app, &data_dir);
        let state = tauri::Manager::state::<AppState>(&app);
        if result.is_ok() {
            *state.install_complete.lock().unwrap() = true;
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
        ready: false,
        fetching: true,
        message: if outstanding == usize::MAX {
            "Downloading card data…".into()
        } else {
            format!("{outstanding} card images still to download")
        },
    }
}

/// Reads a decklist, reports on it, and saves it when it is legal.
#[tauri::command]
fn import_deck(
    state: tauri::State<'_, AppState>,
    name: String,
    text: String,
) -> Result<decks::ImportReport, String> {
    let guard = state.cards.read().unwrap();
    let loaded = guard.as_ref().ok_or("card data is not loaded yet")?;
    decks::import(&deck_store()?, loaded.as_deck_data(), &name, &text)
}

/// A saved deck in the interoperable text format, for the clipboard.
#[tauri::command]
fn export_deck(id: String) -> Result<String, String> {
    let id = op_deck::store::DeckId::new(id).map_err(|e| e.to_string())?;
    deck_store()?
        .load(&id)
        .map(|d| d.to_text())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_deck(id: String) -> Result<(), String> {
    let id = op_deck::store::DeckId::new(id).map_err(|e| e.to_string())?;
    deck_store()?.delete(&id).map_err(|e| e.to_string())
}

#[derive(Serialize)]
struct StartResult {
    snapshot: Snapshot,
    catalogue: Vec<CardInfo>,
}

/// Everything the setup panel decides, as one value.
///
/// A struct rather than a parameter list because the list had grown past what
/// anyone can read at a call site, and a `bool` in seventh position is the kind
/// of argument that gets passed in the wrong order exactly once.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewGameOptions {
    /// `None` asks for a fresh one; the CLI's `--seed` equivalent.
    seed: Option<u64>,
    your_deck: String,
    ai_deck: String,
    difficulty: String,
    you_first: bool,
    /// Play a deck this build cannot fully implement — see below.
    allow_unsupported: bool,
}

#[tauri::command]
fn new_game(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    options: NewGameOptions,
) -> Result<StartResult, String> {
    let NewGameOptions {
        seed,
        your_deck,
        ai_deck,
        difficulty,
        you_first,
        allow_unsupported,
    } = options;

    let guard = state.cards.read().unwrap();
    let loaded = guard.as_ref().ok_or("card data is not loaded yet")?;

    let seed = seed.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    });

    let store = deck_store()?;
    let human = decks::resolve_id(&store, &your_deck)?;
    let ai = decks::resolve_id(&store, &ai_deck)?;

    // A deck with a card whose text nothing implements will play — the card
    // just silently does nothing, which loses games for reasons that look like
    // engine bugs. Blocked by default, and overridable, because testing a deck
    // that is not fully supported yet is a legitimate thing to want.
    if !allow_unsupported {
        for (list, whose) in [(&human, "Your deck"), (&ai, "The opponent's deck")] {
            if !decks::is_supported(loaded.as_deck_data(), list) {
                return Err(format!(
                    "{whose} contains cards this build cannot play as printed. \
                     Tick “allow unsupported cards” to play anyway."
                ));
            }
        }
    }

    let session = Session::new(
        Arc::clone(&loaded.db),
        loaded.scripts(),
        session::SessionConfig {
            seed,
            human_deck: human.clone(),
            ai_deck: ai.clone(),
            human_first: you_first,
            difficulty: Difficulty::parse(&difficulty),
            debug_dir: session::debug_dir_from_env(),
        },
    )
    .map_err(|e| e.to_string())?;

    let catalogue = session.catalogue(&[&human, &ai]);
    let snapshot = session.snapshot();
    *state.session.lock().unwrap() = Some(session);

    // With the human going second the AI opens, which is a full search.
    if snapshot.thinking {
        spawn_ai_turn(app);
    }

    Ok(StartResult {
        snapshot,
        catalogue,
    })
}

/// Event carrying a fresh snapshot once the AI has finished thinking.
pub const GAME_UPDATE_EVENT: &str = "game://update";

/// Applies the human's choice and returns immediately.
///
/// The AI's reply is a full search per decision — inline it would freeze the
/// window, which is what a Tauri command does when it blocks. So the board
/// updates twice: once with the human's own move, then again from
/// `game://update` when the worker is done.
#[tauri::command]
fn choose(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    index: usize,
) -> Result<Snapshot, String> {
    let snapshot = {
        let mut guard = state.session.lock().unwrap();
        let session = guard.as_mut().ok_or("no game in progress")?;
        session.apply_human(index)?;
        session.snapshot()
    };

    if snapshot.thinking {
        spawn_ai_turn(app);
    }
    Ok(snapshot)
}

/// Runs the AI's decisions on a worker, then pushes the result to the UI.
///
/// The session lock is taken only inside the worker, so the command that
/// spawned it has already returned and the window stays live throughout.
fn spawn_ai_turn(app: tauri::AppHandle) {
    {
        let state = tauri::Manager::state::<AppState>(&app);
        let mut thinking = state.ai_thinking.lock().unwrap();
        if *thinking {
            return; // a turn is already being computed
        }
        *thinking = true;
    }

    std::thread::spawn(move || {
        let state = tauri::Manager::state::<AppState>(&app);
        let snapshot = {
            let mut guard = state.session.lock().unwrap();
            match guard.as_mut() {
                Some(session) => {
                    session.run_ai();
                    Some(session.snapshot())
                }
                None => None,
            }
        };
        *state.ai_thinking.lock().unwrap() = false;

        if let Some(snapshot) = snapshot {
            // A window that has gone away is not an error worth surfacing.
            let _ = tauri::Emitter::emit(&app, GAME_UPDATE_EVENT, snapshot);
        }
    });
}

#[tauri::command]
fn snapshot(state: tauri::State<'_, AppState>) -> Result<Snapshot, String> {
    let guard = state.session.lock().unwrap();
    let session = guard.as_ref().ok_or("no game in progress")?;
    Ok(session.snapshot())
}

#[derive(Serialize)]
struct DebugInfo {
    path: Option<String>,
    /// A game is still in progress.
    live: bool,
    /// Why the raw log is withheld, if it is.
    withheld: Option<String>,
    /// Raw log lines, only when it is safe to show them.
    entries: Vec<String>,
    summary: Vec<(String, String)>,
}

/// Session diagnostics.
///
/// The raw log is **omniscient** — it records `GameEvent`, so it contains the
/// opponent's hand and every card drawn. Rendering it during a live game would
/// be a cheat button, so it is withheld until the game ends. `OPSIM_DEBUG_UI=1`
/// overrides that for engine work, where seeing both sides is the point.
#[tauri::command]
fn debug_info(state: tauri::State<'_, AppState>) -> DebugInfo {
    let guard = state.session.lock().unwrap();
    let Some(session) = guard.as_ref() else {
        return DebugInfo {
            path: None,
            live: false,
            withheld: Some("no game in progress".into()),
            entries: Vec::new(),
            summary: Vec::new(),
        };
    };

    let snapshot = session.snapshot();
    let live = snapshot.over.is_none();
    let path = session.debug_log_path().map(|p| p.display().to_string());

    let unlocked = !live || std::env::var("OPSIM_DEBUG_UI").is_ok_and(|v| v == "1");
    let entries = match (&path, unlocked) {
        (Some(path), true) => std::fs::read_to_string(path)
            .map(|t| t.lines().map(str::to_string).collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    };

    DebugInfo {
        path,
        live,
        withheld: (!unlocked).then(|| {
            "The raw log records both hands, so it stays hidden until the game \
             ends. Set OPSIM_DEBUG_UI=1 to override."
                .into()
        }),
        entries,
        summary: vec![
            ("turn".into(), snapshot.view.turn.to_string()),
            ("phase".into(), format!("{:?}", snapshot.view.phase)),
            ("your life".into(), snapshot.view.you.life_count.to_string()),
            (
                "opponent life".into(),
                snapshot.view.opponent.life_count.to_string(),
            ),
            (
                "result".into(),
                snapshot
                    .over
                    .clone()
                    .unwrap_or_else(|| "in progress".into()),
            ),
        ],
    }
}

/// The directory session logs are written to.
#[tauri::command]
fn log_dir(state: tauri::State<'_, AppState>) -> String {
    state.data_dir.join("debug").display().to_string()
}

/// Opens the log directory in the system file manager.
///
/// A convenience, not a dependency: if the platform command is missing the
/// path is still shown in the UI, so nothing is lost.
#[tauri::command]
fn open_log_dir(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let dir = state.data_dir.join("debug");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program)
        .arg(&dir)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("could not open {}: {e}", dir.display()))
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
    let data_dir = ingest::data_dir();

    // Card data is deliberately *not* loaded here. It may not exist yet, and a
    // window that opens and explains itself beats a process that exits.
    tauri::Builder::default()
        .manage(AppState {
            data_dir,
            cards: RwLock::new(None),
            ingesting: Mutex::new(false),
            ai_thinking: Mutex::new(false),
            install_complete: Mutex::new(false),
            session: Mutex::new(None),
            art: Mutex::new(HashMap::new()),
        })
        .invoke_handler(tauri::generate_handler![
            bootstrap,
            decks,
            import_deck,
            export_deck,
            delete_deck,
            new_game,
            choose,
            snapshot,
            card_art,
            debug_info,
            log_dir,
            open_log_dir
        ])
        .run(tauri::generate_context!())
        .expect("failed to start the desktop app");
}
