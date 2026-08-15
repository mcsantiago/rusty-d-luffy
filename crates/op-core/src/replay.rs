//! Per-session debug logs, and the replay that makes them worth writing.
//!
//! A game is a pure function of `(GameConfig, seed, [Action])`, so a log that
//! records the config and every action is not merely a trace — it is a complete
//! reproducer. [`SessionLog`] writes one; [`read`] parses it back and
//! [`SessionRecord::verify`] rebuilds the game from it and checks every step
//! against what was recorded.
//!
//! The per-step `state_hash` is what makes the round trip more than crash
//! triage. Replaying a recorded session reports the *first* step whose hash
//! moved, which turns every log ever written into a regression test against
//! later rules changes: the divergent step names the rule that changed.
//!
//! Written as JSON Lines: one header record, then one record per step. That
//! survives truncation, which matters because the interesting case is a log
//! from a run that crashed or was killed — so the reader drops a partial final
//! line and says so, rather than refusing the file.
//!
//! Every record below is written borrowed and read owned, through `Cow`. One
//! definition serving both directions is deliberate: a separate set of reader
//! structs would be free to drift from the writer's field names, and the
//! failure mode of that drift is a log that still parses and replays as a
//! different game.
//!
//! The log is **omniscient** — it records `GameEvent`, not `PlayerEvent`. It is
//! a local debugging artefact, never sent anywhere, and a redacted log would be
//! useless for diagnosing the engine. It follows that a debug log must not be
//! shown to a player mid-game, and that everything here is a local tool: no
//! part of this module may be reachable from a client.

use std::borrow::Cow;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::action::{Action, IllegalAction};
use crate::card::CardDb;
use crate::event::GameEvent;
use crate::game::{DeckList, Game, GameConfig, SetupError, StepOutcome};
use crate::ids::PlayerId;
use crate::script::ScriptSource;
use crate::state::GameState;

/// The first record in a log: everything needed to rebuild the `GameConfig`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Header<'a> {
    kind: Cow<'a, str>,
    /// Log format version.
    ///
    /// Present so a format change reports itself rather than surfacing as a
    /// serde type error on whichever field happened to move. Absent in logs
    /// written before versioning, which is what `0` means.
    #[serde(default)]
    pub version: u32,
    /// Wall-clock start, seconds since the epoch. Only for ordering files.
    pub started_at: u64,
    pub seed: u64,
    pub first_player: u8,
    pub decks: [DeckSummary<'a>; 2],
    /// Whether deck validation was waived. Part of the config, so a log from a
    /// deliberately illegal deck replays instead of failing setup.
    #[serde(default)]
    pub allow_illegal_decks: bool,
    /// The card-data revision this *build* was pinned to (`SOURCE_REF`, or the
    /// env override) when the session ran.
    ///
    /// Not necessarily the revision the data on disk was fetched at — nothing
    /// in `data/` records that, and the client does not refetch when the pin
    /// moves. So a matching value here is weaker evidence than it looks: it
    /// rules out a pin bump between recording and replay, not a stale cache on
    /// either machine.
    #[serde(default)]
    pub card_data_ref: Option<Cow<'a, str>>,
    /// The engine that recorded this, as `CARGO_PKG_VERSION`.
    ///
    /// A rules change makes old logs diverge, which is the tool working; this
    /// says so rather than leaving it indistinguishable from a bug. Absent in
    /// logs written before the field existed.
    #[serde(default)]
    pub engine_version: Option<Cow<'a, str>>,
    pub notes: Vec<String>,
}

/// The format this build writes, and the only one it can replay.
pub const FORMAT_VERSION: u32 = 1;

/// The engine version stamped into every log this build writes.
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// A decklist, in the order the cards were listed.
///
/// The order is load-bearing, not presentation: instance ids are assigned by
/// walking this list at setup, so a list rebuilt in a different order produces
/// a game where every `CardInstanceId` in the recorded actions names a
/// different card. Storing counts instead of the sequence would round-trip
/// only for lists that happen to be written grouped.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeckSummary<'a> {
    pub leader: Cow<'a, str>,
    pub cards: Vec<Cow<'a, str>>,
}

impl DeckSummary<'_> {
    fn to_decklist(&self) -> DeckList {
        DeckList {
            leader: self.leader.clone().into_owned(),
            cards: self.cards.iter().map(|c| c.clone().into_owned()).collect(),
        }
    }
}

/// An instance id resolved to the card it actually is.
///
/// Traces are written against runtime ids, which say nothing on their own. A
/// reader should not have to reconstruct the game to find out what `card=84`
/// was, so every id a record mentions is resolved here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedCard {
    pub instance: u32,
    pub definition: String,
    pub name: String,
}

/// One recorded step: the action taken, what it produced, and where it left
/// the game.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step<'a> {
    kind: Cow<'a, str>,
    pub n: u64,
    /// `None` for the setup record, which has events but no action.
    pub action: Option<Cow<'a, Action>>,
    pub events: Cow<'a, [GameEvent]>,
    /// Structural hash after the step. A replay that diverges shows up here
    /// before it shows up as wrong behaviour.
    pub state_hash: u64,
    pub turn: u32,
    pub turn_player: u8,
    pub phase: String,
    /// Who owes the next decision, if anyone. Human-facing.
    pub pending: Option<String>,
    pub game_over: Option<String>,
    /// The same two, as the exact `Debug` form a replay can compare against.
    /// Separate from the fields above so the readable ones stay free to change
    /// without silently weakening the check.
    #[serde(default)]
    pub pending_repr: String,
    #[serde(default)]
    pub game_over_repr: String,
    /// Every instance id mentioned above, resolved. Includes the current
    /// battle's participants, so a Counter can be checked against the card
    /// actually under attack without cross-referencing earlier records.
    pub cards: Vec<ResolvedCard>,
    pub battle: Option<BattleRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BattleRef {
    pub step: String,
    pub attacker: u32,
    pub target: u32,
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
    ///
    /// `card_data_ref` identifies the card data in use — the pinned upstream
    /// revision for a real client, `None` for a synthetic pool.
    pub fn create(
        dir: impl AsRef<Path>,
        config: &GameConfig,
        card_data_ref: Option<&str>,
        notes: Vec<String>,
    ) -> std::io::Result<SessionLog> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)?;

        let started_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let seed = config.seed;
        let path = dir.join(format!("session-{started_at}-{seed:016x}.jsonl"));

        let mut writer = BufWriter::new(File::create(&path)?);
        let header = Header {
            kind: Cow::Borrowed(HEADER),
            version: FORMAT_VERSION,
            started_at,
            seed,
            first_player: config.first_player.0,
            decks: [summarise(&config.decks[0]), summarise(&config.decks[1])],
            allow_illegal_decks: config.allow_illegal_decks,
            card_data_ref: card_data_ref.map(Cow::Borrowed),
            engine_version: Some(Cow::Borrowed(ENGINE_VERSION)),
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
    pub fn record(
        &mut self,
        action: Option<&Action>,
        events: &[GameEvent],
        state: &GameState,
        db: &CardDb,
    ) {
        let Some(writer) = self.writer.as_mut() else {
            return;
        };
        self.steps += 1;

        // Resolve every id this record mentions, plus the battle participants.
        let mut ids: Vec<u32> = Vec::new();
        let mut note = |id: crate::ids::CardInstanceId| {
            if !ids.contains(&id.0) {
                ids.push(id.0);
            }
        };
        if let Some(action) = action {
            for id in action_ids(action) {
                note(id);
            }
        }
        for event in events {
            for id in event_ids(event) {
                note(id);
            }
        }
        if let Some(b) = &state.battle {
            note(b.attacker);
            note(b.target);
        }

        let cards = ids
            .iter()
            .filter(|&&i| (i as usize) < state.cards.len())
            .map(|&i| {
                let def = db.get(state.cards[i as usize].def);
                ResolvedCard {
                    instance: i,
                    definition: def.number.clone(),
                    name: def.name.clone(),
                }
            })
            .collect();

        let step = Step {
            kind: Cow::Borrowed(STEP),
            n: self.steps,
            action: action.map(Cow::Borrowed),
            events: Cow::Borrowed(events),
            state_hash: state.state_hash(),
            turn: state.turn,
            turn_player: state.turn_player.0,
            phase: format!("{:?}", state.phase),
            pending: state.pending.as_ref().map(|p| format!("{p:?}")),
            game_over: state.game_over.map(|r| format!("{r:?}")),
            pending_repr: format!("{:?}", state.pending),
            game_over_repr: format!("{:?}", state.game_over),
            cards,
            battle: state.battle.as_ref().map(|b| BattleRef {
                step: format!("{:?}", b.step),
                attacker: b.attacker.0,
                target: b.target.0,
            }),
        };

        let line = match serde_json::to_string(&step) {
            Ok(line) => line,
            Err(_) => return,
        };
        // Flushed every step: a log that loses its tail is worthless for
        // diagnosing a crash, which is the case it exists for.
        if writeln!(writer, "{line}")
            .and_then(|_| writer.flush())
            .is_err()
        {
            self.writer = None;
        }
    }
}

const HEADER: &str = "header";
const STEP: &str = "step";

/// A parsed log, ready to replay.
#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub header: Header<'static>,
    pub steps: Vec<Step<'static>>,
    /// The final line was incomplete and was dropped — the session was killed
    /// mid-write. Replay still runs, it just ends where the log does.
    pub truncated: bool,
    pub path: PathBuf,
}

/// Why a log could not be parsed.
#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is empty: a log begins with a header record")]
    Empty { path: PathBuf },
    #[error(
        "log format version {found}, this build replays version {expected}; \
         re-record it or use a matching build"
    )]
    Version { expected: u32, found: u32 },
    #[error("{path} line {line}: {source}")]
    Malformed {
        path: PathBuf,
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error("{path} line {line}: expected a {expected:?} record, found {found:?}")]
    WrongKind {
        path: PathBuf,
        line: usize,
        expected: &'static str,
        found: String,
    },
    #[error("{path}: first_player is {seat}, but there are only two seats")]
    BadSeat { path: PathBuf, seat: u8 },
}

/// Parses a session log.
///
/// A partial final line is dropped rather than rejected: a log written by a
/// process that crashed is exactly the log worth reading, and JSON Lines is
/// the format precisely so that the surviving prefix stays usable.
pub fn read(path: impl AsRef<Path>) -> Result<SessionRecord, ReadError> {
    let path = path.as_ref().to_path_buf();
    let file = File::open(&path).map_err(|source| ReadError::Io {
        path: path.clone(),
        source,
    })?;

    let mut lines = Vec::new();
    for line in BufReader::new(file).lines() {
        lines.push(line.map_err(|source| ReadError::Io {
            path: path.clone(),
            source,
        })?);
    }
    // A trailing newline yields no entry, so anything left blank is padding.
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    if lines.is_empty() {
        return Err(ReadError::Empty { path });
    }

    let header = parse_header(&path, &lines[0])?;
    if header.first_player > 1 {
        return Err(ReadError::BadSeat {
            path,
            seat: header.first_player,
        });
    }

    let mut steps = Vec::with_capacity(lines.len() - 1);
    let mut truncated = false;
    for (i, line) in lines.iter().enumerate().skip(1) {
        match parse::<Step<'static>>(&path, i + 1, line, STEP) {
            Ok(step) => steps.push(step),
            // Only the last line may be a casualty of a killed process;
            // a bad line anywhere else is a corrupt file, not a truncated one.
            Err(e) => {
                if i + 1 == lines.len() && matches!(e, ReadError::Malformed { .. }) {
                    truncated = true;
                } else {
                    return Err(e);
                }
            }
        }
    }

    Ok(SessionRecord {
        header,
        steps,
        truncated,
        path,
    })
}

/// Parses the header, deciding the format version before anything that depends
/// on the schema.
///
/// The order is the whole point. Deserializing `Header` first means a log whose
/// schema moved dies on whichever field moved — the reader is told
/// `invalid type: integer 50, expected a sequence` when what it needs to hear is
/// that the log is from an older format. So only `kind` and `version` are read
/// up front, and neither may ever gain a requirement that a foreign header
/// might not satisfy.
fn parse_header(path: &Path, line: &str) -> Result<Header<'static>, ReadError> {
    let malformed = |source| ReadError::Malformed {
        path: path.to_path_buf(),
        line: 1,
        source,
    };

    let probe: HeaderProbe = serde_json::from_str(line).map_err(malformed)?;
    // Kind before version: a step record sitting where the header belongs has
    // no version field, and reporting that as "version 0" would be a lie.
    if probe.kind != HEADER {
        return Err(ReadError::WrongKind {
            path: path.to_path_buf(),
            line: 1,
            expected: HEADER,
            found: probe.kind,
        });
    }
    if probe.version != FORMAT_VERSION {
        return Err(ReadError::Version {
            expected: FORMAT_VERSION,
            found: probe.version,
        });
    }

    serde_json::from_str(line).map_err(malformed)
}

/// Parses one record and checks its `kind` tag.
///
/// The tag is what distinguishes a header from a step, so a file whose records
/// are in the wrong order is caught here rather than replaying as nonsense.
fn parse<T>(path: &Path, line_no: usize, line: &str, expected: &'static str) -> Result<T, ReadError>
where
    T: serde::de::DeserializeOwned,
{
    let kind: Kind = serde_json::from_str(line).map_err(|source| ReadError::Malformed {
        path: path.to_path_buf(),
        line: line_no,
        source,
    })?;
    if kind.kind != expected {
        return Err(ReadError::WrongKind {
            path: path.to_path_buf(),
            line: line_no,
            expected,
            found: kind.kind,
        });
    }
    serde_json::from_str(line).map_err(|source| ReadError::Malformed {
        path: path.to_path_buf(),
        line: line_no,
        source,
    })
}

#[derive(Deserialize)]
struct Kind {
    kind: String,
}

/// The only two header fields this build may assume a foreign log has.
///
/// This is the format's compatibility contract: every field here must parse out
/// of *any* header this project has ever written, including ones predating the
/// version stamp. Adding a field re-creates the failure the probe exists to
/// prevent, so it stays at two.
#[derive(Deserialize)]
struct HeaderProbe {
    kind: String,
    #[serde(default)]
    version: u32,
}

/// Where two event lists first disagree.
///
/// Equal lengths with different contents is the case that matters: reporting
/// only the counts reads as though nothing is wrong.
fn first_difference(expected: &[GameEvent], actual: &[GameEvent]) -> String {
    match expected.iter().zip(actual).position(|(e, a)| e != a) {
        Some(i) => format!(
            "first at index {i}: replayed {:?}, recorded {:?}",
            actual[i], expected[i]
        ),
        None if expected.len() == actual.len() => "identical".to_string(),
        None => format!(
            "first {} match, then the lists differ in length",
            expected.len().min(actual.len())
        ),
    }
}

/// A replay that matched the log.
pub struct Verified {
    /// Records replayed, including the setup record.
    pub steps: usize,
    pub final_hash: u64,
    /// The rebuilt game, left where the log ended.
    pub game: Game,
}

/// The first point at which a replay stopped matching the log.
#[derive(Debug, thiserror::Error)]
pub enum Divergence {
    #[error("setup failed: {0}")]
    Setup(#[from] SetupError),
    #[error("step {step}: the engine rejected the recorded action ({source})")]
    Rejected {
        step: u64,
        action: Box<Action>,
        #[source]
        source: IllegalAction,
    },
    #[error("step {step}: state hash {actual:#018x}, recorded {expected:#018x}")]
    Hash {
        step: u64,
        action: Option<Box<Action>>,
        expected: u64,
        actual: u64,
    },
    #[error(
        "step {step}: events differ ({}); replayed {}, recorded {}",
        first_difference(expected, actual),
        actual.len(),
        expected.len()
    )]
    Events {
        step: u64,
        action: Option<Box<Action>>,
        expected: Vec<GameEvent>,
        actual: Vec<GameEvent>,
    },
    #[error("step {step}: {field} is {actual:?}, recorded {expected:?}")]
    State {
        step: u64,
        action: Option<Box<Action>>,
        field: &'static str,
        expected: String,
        actual: String,
    },
    #[error("step {step}: the log records no action, but setup was already replayed")]
    MissingAction { step: u64 },
}

impl SessionRecord {
    /// The config this session ran under.
    pub fn config(&self) -> GameConfig {
        GameConfig {
            seed: self.header.seed,
            // Bounds-checked by `read`.
            first_player: PlayerId(self.header.first_player),
            decks: [
                self.header.decks[0].to_decklist(),
                self.header.decks[1].to_decklist(),
            ],
            allow_illegal_decks: self.header.allow_illegal_decks,
        }
    }

    /// Rebuilds the game and replays every recorded action into it.
    ///
    /// Both the events and the state hash are checked at each step. Events are
    /// checked too because a change that alters what the engine reports without
    /// altering the position is still a change a client would see.
    pub fn verify(
        &self,
        db: Arc<CardDb>,
        scripts: Arc<dyn ScriptSource + Send + Sync>,
    ) -> Result<Verified, Divergence> {
        let (mut game, opening) = Game::new(self.config(), db, scripts)?;

        // Consumed by the one actionless record, which describes the position
        // `Game::new` left. Taking it means a second such record is caught
        // rather than silently re-checked against a stale opening.
        let mut opening = Some(opening);
        let mut replayed = 0usize;
        for record in &self.steps {
            let outcome = match &record.action {
                None => opening
                    .take()
                    .ok_or(Divergence::MissingAction { step: record.n })?,
                Some(action) => {
                    let action = action.clone().into_owned();
                    opening = None;
                    game.step(action.clone())
                        .map_err(|source| Divergence::Rejected {
                            step: record.n,
                            action: Box::new(action),
                            source,
                        })?
                }
            };
            check(record, &outcome, &game)?;
            replayed += 1;
        }

        Ok(Verified {
            steps: replayed,
            final_hash: game.state.state_hash(),
            game,
        })
    }
}

/// Compares one replayed step against what was recorded.
///
/// The hash is checked first: it is the coarser signal, and a position that
/// moved explains any event difference that comes with it.
fn check(record: &Step, outcome: &StepOutcome, game: &Game) -> Result<(), Divergence> {
    let action = || {
        record
            .action
            .as_ref()
            .map(|a| Box::new(a.clone().into_owned()))
    };

    let actual = game.state.state_hash();
    if actual != record.state_hash {
        return Err(Divergence::Hash {
            step: record.n,
            action: action(),
            expected: record.state_hash,
            actual,
        });
    }
    if outcome.events != record.events.as_ref() {
        return Err(Divergence::Events {
            step: record.n,
            action: action(),
            expected: record.events.to_vec(),
            actual: outcome.events.clone(),
        });
    }

    // The hash is not enough on its own. `GameState::state_hash` folds only the
    // pending decision's discriminant and owner, and only whether the game is
    // over — so a change to a `Pending` payload, or to *why* someone lost,
    // matches on hash while being exactly the kind of regression an old log is
    // kept to catch.
    let position: [(&'static str, String, &String); 3] = [
        ("phase", format!("{:?}", game.state.phase), &record.phase),
        (
            "pending",
            format!("{:?}", game.state.pending),
            &record.pending_repr,
        ),
        (
            "game_over",
            format!("{:?}", game.state.game_over),
            &record.game_over_repr,
        ),
    ];
    for (field, actual, expected) in position {
        if &actual != expected {
            return Err(Divergence::State {
                step: record.n,
                action: action(),
                field,
                expected: expected.clone(),
                actual,
            });
        }
    }
    if game.state.turn != record.turn {
        return Err(Divergence::State {
            step: record.n,
            action: action(),
            field: "turn",
            expected: record.turn.to_string(),
            actual: game.state.turn.to_string(),
        });
    }
    Ok(())
}

fn action_ids(action: &Action) -> Vec<crate::ids::CardInstanceId> {
    match action {
        Action::PlayCard { card, replacing } => {
            let mut ids = vec![*card];
            ids.extend(replacing.iter().copied());
            ids
        }
        Action::ActivateEffect { card, discard, .. } => {
            let mut ids = vec![*card];
            ids.extend(discard.iter().copied());
            ids
        }
        Action::GiveDon { to } => vec![*to],
        Action::Attack { attacker, target } => vec![*attacker, *target],
        Action::Block { blocker: Some(c) } => vec![*c],
        Action::Counter { card, to } | Action::CounterEvent { card, to } => vec![*card, *to],
        Action::Choose { cards } | Action::ReturnDon { dons: cards } => cards.clone(),
        Action::Arrange { top, bottom } => top.iter().chain(bottom).copied().collect(),
        _ => Vec::new(),
    }
}

fn event_ids(event: &GameEvent) -> Vec<crate::ids::CardInstanceId> {
    use GameEvent as E;
    match event {
        E::Drew { card, .. }
        | E::CardPlayed { card, .. }
        | E::CardMoved { card, .. }
        | E::Rested { card }
        | E::SetActive { card }
        | E::KnockedOut { card }
        | E::LifeTaken { card, .. }
        | E::TriggerActivated { card, .. } => vec![*card],
        E::DonGiven { don, to, .. } => vec![*don, *to],
        E::DonDetached { don, from, .. } => vec![*don, *from],
        E::AttackDeclared { attacker, target } => vec![*attacker, *target],
        E::Blocked { blocker, replacing } => vec![*blocker, *replacing],
        E::Countered { card, target, .. } => vec![*card, *target],
        E::BattleResolved {
            attacker, target, ..
        } => vec![*attacker, *target],
        E::EffectActivated { source, .. } | E::NoLegalTargets { source, .. } => vec![*source],
        _ => Vec::new(),
    }
}

fn summarise(deck: &DeckList) -> DeckSummary<'_> {
    DeckSummary {
        leader: Cow::Borrowed(&deck.leader),
        cards: deck
            .cards
            .iter()
            .map(|c| Cow::Borrowed(c.as_str()))
            .collect(),
    }
}
