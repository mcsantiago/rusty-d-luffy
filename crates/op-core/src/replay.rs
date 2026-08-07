//! Per-session debug logs.
//!
//! A game is a pure function of `(GameConfig, seed, [Action])`, so a log that
//! records the seed and every action is not merely a trace — it is a complete
//! reproducer. Anything that went wrong can be replayed exactly.
//!
//! Written as JSON Lines: one header record, then one record per step. That
//! survives truncation, which matters because the interesting case is a log
//! from a run that crashed or was killed.
//!
//! The log is **omniscient** — it records `GameEvent`, not `PlayerEvent`. It is
//! a local debugging artefact, never sent anywhere, and a redacted log would be
//! useless for diagnosing the engine. It follows that a debug log must not be
//! shown to a player mid-game.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::action::Action;
use crate::event::GameEvent;
use crate::game::DeckList;
use crate::state::GameState;

#[derive(Serialize)]
struct Header<'a> {
    kind: &'static str,
    /// Wall-clock start, seconds since the epoch. Only for ordering files.
    started_at: u64,
    seed: u64,
    first_player: u8,
    decks: [DeckSummary<'a>; 2],
    notes: Vec<String>,
}

#[derive(Serialize)]
struct DeckSummary<'a> {
    leader: &'a str,
    cards: usize,
    /// Card numbers with counts, so the exact list can be rebuilt.
    list: Vec<(String, usize)>,
}

#[derive(Serialize)]
struct Step<'a> {
    kind: &'static str,
    n: u64,
    /// `None` for the setup record, which has events but no action.
    action: Option<&'a Action>,
    events: &'a [GameEvent],
    /// Structural hash after the step. A replay that diverges shows up here
    /// before it shows up as wrong behaviour.
    state_hash: u64,
    turn: u32,
    turn_player: u8,
    phase: String,
    /// Who owes the next decision, if anyone.
    pending: Option<String>,
    game_over: Option<String>,
}

/// A debug log for one session.
///
/// Every write is best-effort: a failure to log must never disturb the game, so
/// errors disable further writing rather than propagating.
pub struct SessionLog {
    writer: Option<BufWriter<File>>,
    path: PathBuf,
    steps: u64,
}

impl SessionLog {
    /// Creates `<dir>/session-<timestamp>-<seed>.jsonl`.
    pub fn create(
        dir: impl AsRef<Path>,
        seed: u64,
        first_player: crate::ids::PlayerId,
        decks: &[DeckList; 2],
        notes: Vec<String>,
    ) -> std::io::Result<SessionLog> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)?;

        let started_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let path = dir.join(format!("session-{started_at}-{seed:016x}.jsonl"));

        let mut writer = BufWriter::new(File::create(&path)?);
        let header = Header {
            kind: "header",
            started_at,
            seed,
            first_player: first_player.0,
            decks: [summarise(&decks[0]), summarise(&decks[1])],
            notes,
        };
        writeln!(writer, "{}", serde_json::to_string(&header)?)?;
        writer.flush()?;

        Ok(SessionLog {
            writer: Some(writer),
            path,
            steps: 0,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Records one step. `action` is `None` for the setup record.
    pub fn record(&mut self, action: Option<&Action>, events: &[GameEvent], state: &GameState) {
        let Some(writer) = self.writer.as_mut() else {
            return;
        };
        self.steps += 1;

        let step = Step {
            kind: "step",
            n: self.steps,
            action,
            events,
            state_hash: state.state_hash(),
            turn: state.turn,
            turn_player: state.turn_player.0,
            phase: format!("{:?}", state.phase),
            pending: state.pending.as_ref().map(|p| format!("{p:?}")),
            game_over: state.game_over.map(|r| format!("{r:?}")),
        };

        let line = match serde_json::to_string(&step) {
            Ok(line) => line,
            Err(_) => return,
        };
        // Flushed every step: a log that loses its tail is worthless for
        // diagnosing a crash, which is the case it exists for.
        if writeln!(writer, "{line}").and_then(|_| writer.flush()).is_err() {
            self.writer = None;
        }
    }
}

fn summarise(deck: &DeckList) -> DeckSummary<'_> {
    let mut list: Vec<(String, usize)> = Vec::new();
    for number in &deck.cards {
        match list.iter_mut().find(|(n, _)| n == number) {
            Some((_, count)) => *count += 1,
            None => list.push((number.clone(), 1)),
        }
    }
    DeckSummary {
        leader: &deck.leader,
        cards: deck.cards.len(),
        list,
    }
}
