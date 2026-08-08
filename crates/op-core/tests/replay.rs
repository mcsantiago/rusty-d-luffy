//! The session log is only worth writing if it reads back as the same game.
//!
//! These tests exercise the round trip the debug log exists for: write a real
//! playout, parse the file, rebuild the game from the header alone, and step
//! the recorded actions back through the engine.

mod common;

use std::sync::Arc;

use common::{deck_of, TestCards, TestScripts};
use op_core::replay::{self, Divergence};
use op_core::script::ScriptSource;
use op_core::{legal_actions, DeckList, Game, GameConfig, PlayerId, SessionLog};

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

/// A scratch directory that removes itself, so a failing test does not leave
/// logs behind and a passing one does not accumulate them.
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        // The thread id keeps parallel tests in this binary off each other.
        let dir = std::env::temp_dir().join(format!(
            "opsim-replay-{}-{name}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("scratch dir");
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn decklist(leader: &str, cards: Vec<&str>) -> DeckList {
    DeckList {
        leader: leader.to_string(),
        cards: cards.into_iter().map(|c| c.to_string()).collect(),
    }
}

fn scripts() -> Arc<dyn ScriptSource + Send + Sync> {
    Arc::new(TestScripts::default())
}

/// Plays a game to completion, logging every step, and returns the log path
/// alongside the position it finished in.
fn logged_playout(
    dir: &TempDir,
    config: GameConfig,
    policy_seed: u64,
) -> (std::path::PathBuf, u64) {
    let cards = TestCards::new();
    let db = Arc::new(cards.db.clone());

    let mut log = SessionLog::create(&dir.0, &config, Some("test-pool"), vec!["test".into()])
        .expect("log should be created");
    let path = log.path().to_path_buf();

    let (mut game, opening) =
        Game::new(config, Arc::clone(&db), scripts()).expect("setup should succeed");
    log.record(None, &opening.events, &game.state, &db);

    let mut policy = StdRng::seed_from_u64(policy_seed);
    for _ in 0..4000 {
        if game.is_over() || game.pending().is_none() {
            break;
        }
        let legal = legal_actions(&game);
        let action = legal[policy.gen_range(0..legal.len())].clone();
        let outcome = game
            .step(action.clone())
            .unwrap_or_else(|e| panic!("legal action {action:?} was rejected: {e}"));
        log.record(Some(&action), &outcome.events, &game.state, &db);
    }

    (path, game.state.state_hash())
}

fn config_for(seed: u64, p0: DeckList, p1: DeckList) -> GameConfig {
    GameConfig {
        seed,
        first_player: PlayerId::P0,
        decks: [p0, p1],
        // The test pool's decks are 40 cards, not 50.
        allow_illegal_decks: true,
    }
}

/// The whole point of the format: a log rebuilds the game it recorded.
#[test]
fn a_log_replays_into_the_game_it_recorded() {
    let dir = TempDir::new("roundtrip");
    let config = config_for(
        11,
        decklist("LDR-001", deck_of("CHR-5K", 40)),
        decklist("LDR-002", deck_of("CHR-BLOCK", 40)),
    );
    let (path, final_hash) = logged_playout(&dir, config, 7);

    let record = replay::read(&path).expect("log should parse");
    assert!(!record.truncated);
    assert!(record.steps.len() > 5, "expected a real game");
    assert_eq!(record.header.card_data_ref.as_deref(), Some("test-pool"));

    let cards = TestCards::new();
    let verified = record
        .verify(Arc::new(cards.db.clone()), scripts())
        .expect("log should replay");

    assert_eq!(verified.final_hash, final_hash);
    assert_eq!(verified.steps, record.steps.len());
}

/// Instance ids are assigned by walking the decklist at setup, so the header
/// has to store the *sequence*, not a tally. A header that recorded counts
/// would rebuild this deck grouped, every id would shift, and the recorded
/// actions would silently address different cards.
///
/// The deck below interleaves its copies precisely so that a grouped rebuild
/// is a different deck. Collapsing the list to counts fails this test.
#[test]
fn an_interleaved_decklist_survives_the_round_trip() {
    let dir = TempDir::new("interleaved");
    let interleaved: Vec<&str> = std::iter::repeat_n(["CHR-5K", "CHR-2K", "CHR-BLOCK"], 13)
        .flatten()
        .take(39)
        .collect();
    let config = config_for(
        3,
        decklist("LDR-001", interleaved.clone()),
        decklist("LDR-002", deck_of("CHR-5K", 40)),
    );
    let (path, final_hash) = logged_playout(&dir, config, 21);

    let record = replay::read(&path).expect("log should parse");

    // The recorded list is the sequence as written, not a regrouping of it.
    let recorded: Vec<&str> = record.header.decks[0]
        .cards
        .iter()
        .map(|c| c.as_ref())
        .collect();
    assert_eq!(recorded, interleaved);

    let cards = TestCards::new();
    let verified = record
        .verify(Arc::new(cards.db.clone()), scripts())
        .expect("an interleaved decklist should replay");
    assert_eq!(verified.final_hash, final_hash);
}

/// A replay that cannot fail is not a check. Corrupting one recorded hash must
/// be caught, at that step and no earlier.
#[test]
fn a_tampered_hash_is_reported_at_the_step_it_changed() {
    let dir = TempDir::new("tampered-hash");
    let config = config_for(
        5,
        decklist("LDR-001", deck_of("CHR-5K", 40)),
        decklist("LDR-002", deck_of("CHR-BLOCK", 40)),
    );
    let (path, _) = logged_playout(&dir, config, 9);

    let text = std::fs::read_to_string(&path).unwrap();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    assert!(
        lines.len() > 4,
        "need a few steps to corrupt one in the middle"
    );

    // Rewrite step 3's hash, leaving everything else intact.
    let target = 3;
    let mut step: serde_json::Value = serde_json::from_str(&lines[target]).unwrap();
    let recorded = step["state_hash"].as_u64().unwrap();
    step["state_hash"] = serde_json::json!(recorded ^ 0xDEAD_BEEF);
    let n = step["n"].as_u64().unwrap();
    lines[target] = serde_json::to_string(&step).unwrap();
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();

    let record = replay::read(&path).expect("a tampered log still parses");
    let cards = TestCards::new();
    match record.verify(Arc::new(cards.db.clone()), scripts()) {
        Err(Divergence::Hash { step, expected, .. }) => {
            assert_eq!(step, n, "the divergence must name the step that moved");
            assert_eq!(expected, recorded ^ 0xDEAD_BEEF);
        }
        Err(other) => panic!("expected a hash divergence, got {other}"),
        Ok(_) => panic!("a corrupted hash replayed clean — the check does nothing"),
    }
}

/// Events are checked as well as the hash: a change that alters what the
/// engine reports without moving the position is still a change.
#[test]
fn a_tampered_event_list_is_reported() {
    let dir = TempDir::new("tampered-events");
    let config = config_for(
        5,
        decklist("LDR-001", deck_of("CHR-5K", 40)),
        decklist("LDR-002", deck_of("CHR-BLOCK", 40)),
    );
    let (path, _) = logged_playout(&dir, config, 9);

    let text = std::fs::read_to_string(&path).unwrap();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();

    // Find a step that emitted events and drop one, leaving the hash alone.
    let target = (1..lines.len())
        .find(|&i| {
            let step: serde_json::Value = serde_json::from_str(&lines[i]).unwrap();
            step["events"].as_array().is_some_and(|e| !e.is_empty())
        })
        .expect("some step emits events");
    let mut step: serde_json::Value = serde_json::from_str(&lines[target]).unwrap();
    let n = step["n"].as_u64().unwrap();
    step["events"].as_array_mut().unwrap().pop();
    lines[target] = serde_json::to_string(&step).unwrap();
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();

    let record = replay::read(&path).expect("log should parse");
    let cards = TestCards::new();
    match record.verify(Arc::new(cards.db.clone()), scripts()) {
        Err(Divergence::Events { step, .. }) => assert_eq!(step, n),
        Err(other) => panic!("expected an event divergence, got {other}"),
        Ok(_) => panic!("a dropped event replayed clean — the check does nothing"),
    }
}

/// JSON Lines is the format precisely so a killed process still leaves a
/// usable log. The surviving prefix must replay, and the reader must say that
/// the tail is missing rather than pretending the game ended there.
#[test]
fn a_log_truncated_mid_record_still_replays_its_prefix() {
    let dir = TempDir::new("truncated");
    let config = config_for(
        13,
        decklist("LDR-001", deck_of("CHR-5K", 40)),
        decklist("LDR-002", deck_of("CHR-BLOCK", 40)),
    );
    let (path, _) = logged_playout(&dir, config, 4);

    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let keep = lines.len() - 1;
    // Half a record, as a kill mid-write would leave it.
    let last = lines[keep];
    let mut truncated = lines[..keep].join("\n");
    truncated.push('\n');
    truncated.push_str(&last[..last.len() / 2]);
    std::fs::write(&path, truncated).unwrap();

    let record = replay::read(&path).expect("a truncated log should still parse");
    assert!(record.truncated, "the dropped tail must be reported");
    assert_eq!(record.steps.len(), keep - 1);

    let cards = TestCards::new();
    let verified = record
        .verify(Arc::new(cards.db.clone()), scripts())
        .expect("the surviving prefix should replay");
    assert_eq!(verified.steps, record.steps.len());
}

/// Corruption in the middle of a file is not truncation, and reading it as
/// though it were would silently drop steps from the replay.
#[test]
fn a_corrupt_record_that_is_not_the_last_is_rejected() {
    let dir = TempDir::new("corrupt");
    let config = config_for(
        17,
        decklist("LDR-001", deck_of("CHR-5K", 40)),
        decklist("LDR-002", deck_of("CHR-BLOCK", 40)),
    );
    let (path, _) = logged_playout(&dir, config, 8);

    let text = std::fs::read_to_string(&path).unwrap();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    lines[2] = "{ not json".into();
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();

    match replay::read(&path) {
        Err(replay::ReadError::Malformed { line, .. }) => assert_eq!(line, 3),
        other => panic!("expected a malformed-record error, got {other:?}"),
    }
}

/// The state hash folds only the pending decision's *discriminant* and owner,
/// so a change to a `Pending` payload matches on hash while being exactly the
/// regression an old log is kept to catch. The position fields close that.
#[test]
fn a_changed_pending_payload_is_caught_even_though_the_hash_matches() {
    let dir = TempDir::new("pending-payload");
    let config = config_for(
        5,
        decklist("LDR-001", deck_of("CHR-5K", 40)),
        decklist("LDR-002", deck_of("CHR-BLOCK", 40)),
    );
    let (path, _) = logged_playout(&dir, config, 9);

    let text = std::fs::read_to_string(&path).unwrap();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();

    // Find a step whose pending decision has a payload worth changing, and
    // rewrite only that — the hash stays exactly as recorded.
    let target = (1..lines.len())
        .find(|&i| {
            serde_json::from_str::<serde_json::Value>(&lines[i])
                .ok()
                .and_then(|s| s["pending_repr"].as_str().map(|r| r.contains('{')))
                .unwrap_or(false)
        })
        .expect("some step has a pending decision");

    let mut step: serde_json::Value = serde_json::from_str(&lines[target]).unwrap();
    let hash_before = step["state_hash"].as_u64().unwrap();
    step["pending_repr"] = serde_json::json!("Pending::Fabricated { player: PlayerId(0) }");
    let n = step["n"].as_u64().unwrap();
    lines[target] = serde_json::to_string(&step).unwrap();
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();

    let record = replay::read(&path).expect("still parses");
    assert_eq!(
        record.steps.iter().find(|s| s.n == n).unwrap().state_hash,
        hash_before,
        "the hash must be untouched, or this proves nothing"
    );

    match record.verify(Arc::new(TestCards::new().db.clone()), scripts()) {
        Err(Divergence::State { step, field, .. }) => {
            assert_eq!(step, n);
            assert_eq!(field, "pending");
        }
        Err(other) => panic!("expected a State divergence, got {other:?}"),
        Ok(_) => panic!("a changed pending payload must not replay clean"),
    }
}

/// A log from a build that wrote a different format should say so, rather than
/// surfacing as a serde type error on whichever field happened to move.
#[test]
fn a_log_from_another_format_version_is_refused_by_name() {
    let dir = TempDir::new("version");
    let config = config_for(
        5,
        decklist("LDR-001", deck_of("CHR-5K", 40)),
        decklist("LDR-002", deck_of("CHR-BLOCK", 40)),
    );
    let (path, _) = logged_playout(&dir, config, 4);

    let text = std::fs::read_to_string(&path).unwrap();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut header: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(
        header["version"],
        replay::FORMAT_VERSION,
        "the writer must stamp the version it writes"
    );
    header["version"] = serde_json::json!(replay::FORMAT_VERSION + 7);
    lines[0] = serde_json::to_string(&header).unwrap();
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();

    match replay::read(&path) {
        Err(err) => {
            let message = err.to_string();
            assert!(
                message.contains("version"),
                "the error should name the version, got: {message}"
            );
        }
        Ok(_) => panic!("a log from an unknown format version must be refused"),
    }
}

/// `Verified::steps` counts what was actually replayed. It previously echoed
/// the record count, so assertions against it could not fail.
#[test]
fn the_replayed_step_count_is_counted_not_echoed() {
    let dir = TempDir::new("counted");
    let config = config_for(
        5,
        decklist("LDR-001", deck_of("CHR-5K", 40)),
        decklist("LDR-002", deck_of("CHR-BLOCK", 40)),
    );
    let (path, _) = logged_playout(&dir, config, 6);

    // Truncating mid-record drops the partial line, so fewer records replay
    // than the file appears to hold — which an echoed count could never show.
    // The first line is the header, so keeping four lines keeps three steps.
    let text = std::fs::read_to_string(&path).unwrap();
    let keep: Vec<&str> = text.lines().take(4).collect();
    std::fs::write(&path, keep.join("\n") + "\n{\"kind\":\"st").unwrap();

    let record = replay::read(&path).expect("a truncated log replays its prefix");
    let verified = record
        .verify(Arc::new(TestCards::new().db.clone()), scripts())
        .expect("the prefix matches");
    assert_eq!(verified.steps, record.steps.len());
    assert_eq!(verified.steps, 3, "header plus three steps: three replayed");
}
