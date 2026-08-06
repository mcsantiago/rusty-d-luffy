//! Desktop client.
//!
//! Tauri rather than Electron because the engine is already Rust: it links into
//! this binary and a UI click calls `Game::step` directly, with no sidecar
//! process and no hand-written IPC protocol. The front end is plain ES modules
//! under `client/`, so there is no JavaScript build step either.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod render;
mod session;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

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

struct AppState {
    db: Arc<CardDb>,
    scripts: Arc<dyn ScriptSource + Send + Sync>,
    session: Mutex<Option<Session>>,
    /// Card art, base64-encoded on first request and kept for the process.
    art: Mutex<HashMap<String, Option<String>>>,
    data_dir: std::path::PathBuf,
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
    let seed = seed.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    });

    let human = deck_by_name(&your_deck);
    let ai = deck_by_name(&ai_deck);

    let session = Session::new(
        Arc::clone(&state.db),
        Arc::clone(&state.scripts),
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
    state
        .art
        .lock()
        .unwrap()
        .insert(number, encoded.clone());
    encoded
}

fn main() {
    // The repo's data/ directory, resolved relative to this crate so the app
    // runs from a checkout without an install step.
    let data_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");

    let db = match CardDb::load_dir(data_dir.join("cards")) {
        Ok(db) => db,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    };
    let scripts: Arc<dyn ScriptSource + Send + Sync> = Arc::new(Cards::new(&db));

    tauri::Builder::default()
        .manage(AppState {
            db: Arc::new(db),
            scripts,
            session: Mutex::new(None),
            art: Mutex::new(HashMap::new()),
            data_dir,
        })
        .invoke_handler(tauri::generate_handler![
            new_game, choose, snapshot, card_art
        ])
        .run(tauri::generate_context!())
        .expect("failed to start the desktop app");
}
