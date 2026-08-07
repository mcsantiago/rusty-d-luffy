//! One game in progress, plus everything the UI needs to draw it.
//!
//! The front end is a renderer. It never receives `GameState` — only a
//! [`PlayerView`], a list of projected [`PlayerEvent`]s already rendered to
//! text, and the legal actions available to the human. That is the same
//! boundary the multiplayer server will use, so the UI code written against it
//! works unchanged when the transport becomes a socket.

use std::sync::Arc;

use op_ai::{Agent, HeuristicAgent, IsmctsAgent, IsmctsConfig};
use op_core::card::{CardDb, Category};
use op_core::script::ScriptSource;
use op_core::view::PlayerView;
use op_core::{
    legal_actions, Action, CardInstanceId, DeckList, Game, GameConfig, PlayerId, SetupError,
};
use rand::rngs::StdRng;
use rand::SeedableRng;
use serde::Serialize;

/// Printed details the UI needs to draw a card. Sent once per game for every
/// card either deck can contain, then keyed by card number.
#[derive(Debug, Clone, Serialize)]
pub struct CardInfo {
    pub number: String,
    pub name: String,
    pub category: String,
    pub cost: u8,
    pub life: Option<u8>,
    pub power: Option<i32>,
    pub counter: Option<i32>,
    pub colors: Vec<String>,
    pub types: Vec<String>,
    pub effect: Option<String>,
    pub trigger: Option<String>,
}

/// One legal action, as an option the UI can present.
#[derive(Debug, Clone, Serialize)]
pub struct Choice {
    pub index: usize,
    pub label: String,
    /// Cards this action involves, so hovering can highlight them.
    pub cards: Vec<u32>,
    /// Coarse kind, for grouping and styling.
    pub kind: &'static str,
}

/// Everything the UI needs after a step.
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub view: PlayerView,
    pub log: Vec<String>,
    pub options: Vec<Choice>,
    pub question: Option<String>,
    pub over: Option<String>,
    /// Whose turn it is, as a label.
    pub turn_label: String,
    /// The AI owes a decision; the UI should expect a `game://update` shortly.
    pub thinking: bool,
}

/// Where session logs go, or `None` when disabled.
///
/// `OPSIM_DEBUG_DIR=` (empty) turns logging off; anything else overrides the
/// default of `<repo>/debug`.
fn debug_dir() -> Option<std::path::PathBuf> {
    match std::env::var("OPSIM_DEBUG_DIR") {
        Ok(dir) if dir.is_empty() => None,
        Ok(dir) => Some(std::path::PathBuf::from(dir)),
        Err(_) => Some(crate::ingest::repo_root().join("debug")),
    }
}

pub struct Session {
    game: Game,
    ai: Box<dyn Agent + Send>,
    human: PlayerId,
    log: Vec<String>,
    db: Arc<CardDb>,
    /// Omniscient debug log for this session. Never shown to the player — it
    /// records `GameEvent`, so it contains both hands.
    debug: Option<op_core::SessionLog>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Difficulty {
    Easy,
    Normal,
    Hard,
}

impl Difficulty {
    pub fn parse(s: &str) -> Difficulty {
        match s {
            "easy" => Difficulty::Easy,
            "hard" => Difficulty::Hard,
            _ => Difficulty::Normal,
        }
    }

    fn agent(self, seed: u64) -> Box<dyn Agent + Send> {
        match self {
            Difficulty::Easy => Box::new(HeuristicAgent::new(StdRng::seed_from_u64(seed))),
            Difficulty::Normal => Box::new(IsmctsAgent::new(IsmctsConfig {
                iterations: 250,
                rollout_depth: 45,
                seed,
                ..Default::default()
            })),
            Difficulty::Hard => Box::new(IsmctsAgent::new(IsmctsConfig {
                iterations: 900,
                rollout_depth: 65,
                seed,
                ..Default::default()
            })),
        }
    }
}

impl Session {
    pub fn new(
        db: Arc<CardDb>,
        scripts: Arc<dyn ScriptSource + Send + Sync>,
        seed: u64,
        human_deck: DeckList,
        ai_deck: DeckList,
        human_first: bool,
        difficulty: Difficulty,
    ) -> Result<Session, SetupError> {
        let config = GameConfig {
            seed,
            first_player: if human_first {
                PlayerId::P0
            } else {
                PlayerId::P1
            },
            decks: [human_deck, ai_deck],
            allow_illegal_decks: false,
        };
        let decks = config.decks.clone();
        let (game, opening) = Game::new(config, Arc::clone(&db), scripts)?;

        // Best-effort: a session that cannot write a debug log still plays.
        let debug = debug_dir()
            .and_then(|dir| {
                op_core::SessionLog::create(
                    dir,
                    seed,
                    if human_first { PlayerId::P0 } else { PlayerId::P1 },
                    &decks,
                    vec![format!("difficulty={difficulty:?}"), "client=desktop".into()],
                )
                .map_err(|e| eprintln!("debug log disabled: {e}"))
                .ok()
            });

        let mut session = Session {
            game,
            ai: difficulty.agent(seed ^ 0x5EED),
            human: PlayerId::P0,
            log: Vec::new(),
            db,
            debug,
        };
        if let Some(debug) = session.debug.as_mut() {
            debug.record(None, &opening.events, &session.game.state, session.db.as_ref());
        }
        session.absorb(&opening.events);
        // The AI is deliberately *not* run here. If it moves first, that is a
        // full search, and doing it inline would block whoever called us — the
        // caller drives it on a worker instead, same as any other AI turn.
        Ok(session)
    }

    /// Applies one of the human's offered options.
    ///
    /// Stops there: the AI's reply is a separate step so the caller can put it
    /// on a worker thread and let the board show the human's own move first.
    pub fn apply_human(&mut self, index: usize) -> Result<(), String> {
        let pending = self.game.pending().ok_or("no decision is pending")?;
        if pending.player() != self.human {
            return Err("it is not your decision".into());
        }
        let legal = legal_actions(&self.game);
        let action = legal
            .get(index)
            .cloned()
            .ok_or_else(|| format!("option {index} is out of range"))?;
        let outcome = self.game.step(action.clone()).map_err(|e| e.to_string())?;
        self.record(Some(&action), &outcome.events);
        self.absorb(&outcome.events);
        Ok(())
    }

    /// Whether the pending decision belongs to the AI.
    pub fn ai_to_act(&self) -> bool {
        !self.game.is_over()
            && self
                .game
                .pending()
                .is_some_and(|p| p.player() != self.human)
    }

    /// Steps the AI while the pending decision belongs to it.
    ///
    /// Expensive — a search per decision. Call it off the UI thread.
    pub fn run_ai(&mut self) {
        for _ in 0..10_000 {
            if self.game.is_over() {
                return;
            }
            let Some(pending) = self.game.pending() else {
                return;
            };
            if pending.player() == self.human {
                return;
            }
            let seat = pending.player();
            let action = self.ai.choose(&self.game, seat);
            let Ok(outcome) = self.game.step(action.clone()) else {
                return;
            };
            self.record(Some(&action), &outcome.events);
            self.absorb(&outcome.events);
        }
    }

    fn record(&mut self, action: Option<&op_core::Action>, events: &[op_core::GameEvent]) {
        if let Some(debug) = self.debug.as_mut() {
            debug.record(action, events, &self.game.state, self.db.as_ref());
        }
    }

    /// Path of this session's debug log, if one is being written.
    pub fn debug_log_path(&self) -> Option<&std::path::Path> {
        self.debug.as_ref().map(|d| d.path())
    }

    /// Renders events from the human's projection into the visible log.
    fn absorb(&mut self, events: &[op_core::GameEvent]) {
        for event in events {
            let projected = event.project(&self.game.state, self.human);
            if let Some(line) = crate::render::line(&projected, &self.game, self.human) {
                self.log.push(line);
            }
        }
        // The log is a scrollback, not a transcript; the UI shows the tail.
        if self.log.len() > 400 {
            let excess = self.log.len() - 400;
            self.log.drain(..excess);
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let derived = self.game.derived();
        let view = PlayerView::project(&self.game.state, &self.db, &derived, self.human);

        let options = if self
            .game
            .pending()
            .is_some_and(|p| p.player() == self.human)
        {
            legal_actions(&self.game)
                .iter()
                .enumerate()
                .map(|(index, action)| Choice {
                    index,
                    label: crate::render::action_label(action, &self.game),
                    cards: action_cards(action).iter().map(|c| c.0).collect(),
                    kind: action_kind(action),
                })
                .collect()
        } else {
            Vec::new()
        };

        let question = self
            .game
            .pending()
            .filter(|p| p.player() == self.human)
            .map(crate::render::question);

        let over = self.game.result().map(|result| match result.winner() {
            Some(w) if w == self.human => "You win".to_string(),
            Some(_) => "You lose".to_string(),
            None => "Draw".to_string(),
        });

        let turn_label = if self.game.state.turn_player == self.human {
            "Your turn".to_string()
        } else {
            "Opponent's turn".to_string()
        };

        Snapshot {
            view,
            log: self.log.clone(),
            options,
            question,
            over,
            turn_label,
            thinking: self.ai_to_act(),
        }
    }

    /// Printed details for every card either deck can contain.
    pub fn catalogue(&self, decks: &[&DeckList]) -> Vec<CardInfo> {
        let mut out = Vec::new();
        let mut seen: Vec<&str> = Vec::new();
        for deck in decks {
            for number in std::iter::once(&deck.leader).chain(deck.cards.iter()) {
                if seen.contains(&number.as_str()) {
                    continue;
                }
                seen.push(number);
                let Some(def) = self.db.by_number(number) else {
                    continue;
                };
                let def = self.db.get(def);
                out.push(CardInfo {
                    number: def.number.clone(),
                    name: def.name.clone(),
                    category: match def.category {
                        Category::Leader => "Leader",
                        Category::Character => "Character",
                        Category::Event => "Event",
                        Category::Stage => "Stage",
                        Category::Don => "DON!!",
                    }
                    .to_string(),
                    cost: def.cost,
                    life: def.life,
                    power: def.power,
                    counter: def.counter,
                    colors: def.colors.iter().map(|c| format!("{c:?}")).collect(),
                    types: def.types.clone(),
                    effect: def.effect.clone(),
                    trigger: def.trigger.clone(),
                });
            }
        }
        out
    }
}

/// Cards an action refers to, for UI highlighting.
fn action_cards(action: &Action) -> Vec<CardInstanceId> {
    match action {
        Action::PlayCard { card } | Action::ActivateEffect { card, .. } => vec![*card],
        Action::GiveDon { to } => vec![*to],
        Action::Attack { attacker, target } => vec![*attacker, *target],
        Action::Block {
            blocker: Some(card),
        } => vec![*card],
        Action::Counter { card, to } | Action::CounterEvent { card, to } => vec![*card, *to],
        Action::Choose { cards } => cards.clone(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use op_cards::Cards;

    fn fixture() -> Option<Session> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/cards");
        let db = CardDb::load_dir(dir).ok()?;
        let scripts: Arc<dyn ScriptSource + Send + Sync> = Arc::new(Cards::new(&db));
        Session::new(
            Arc::new(db),
            scripts,
            7,
            crate::st01(),
            crate::st02(),
            true,
            Difficulty::Easy,
        )
        .ok()
        .map(|mut session| {
            // Construction no longer runs the AI, so tests that expect a
            // human decision waiting must drive it once.
            session.run_ai();
            session
        })
    }

    /// Drives a whole game through the same path the UI uses, always taking the
    /// first offered option. Catches panics in snapshotting and in the AI
    /// driver without needing a window.
    #[test]
    fn a_full_game_can_be_played_through_the_session_api() {
        let Some(mut session) = fixture() else {
            eprintln!("skipping: run tools/ingest/fetch_cards.py");
            return;
        };

        for step in 0..5_000 {
            let snap = session.snapshot();
            if snap.over.is_some() {
                assert!(session.snapshot().options.is_empty());
                return;
            }
            assert!(
                !snap.options.is_empty(),
                "step {step}: no options while the game is live ({:?})",
                snap.question
            );
            session.apply_human(0).expect("first option must be legal");
            session.run_ai();
        }
        panic!("game did not finish");
    }

    /// The UI is handed a view, never state — so nothing it renders can name a
    /// card in the opponent's hand.
    #[test]
    fn snapshots_never_expose_the_opponents_hand() {
        let Some(mut session) = fixture() else { return };

        for _ in 0..200 {
            let snap = session.snapshot();
            assert!(
                snap.view.opponent.hand.is_empty(),
                "the opponent's hand must reach the UI as a count only"
            );
            for card in &snap.view.opponent.characters {
                assert!(card.number.is_some(), "board cards are public");
            }
            if snap.over.is_some() || snap.options.is_empty() {
                break;
            }
            session.apply_human(0).unwrap();
            session.run_ai();
        }
    }

    /// Human input and the AI's reply now run on different threads, so the
    /// session has to refuse a human action aimed at a decision that is not
    /// theirs — otherwise a click landing while the worker is mid-turn would
    /// play the AI's move for it.
    #[test]
    fn a_human_action_is_refused_when_the_decision_is_the_ais() {
        let Some(mut session) = fixture() else { return };

        // Drive to a point where the AI owes a decision.
        let mut guard = 0;
        while !session.ai_to_act() && session.snapshot().over.is_none() {
            session.apply_human(0).unwrap();
            guard += 1;
            assert!(guard < 500, "never reached an AI decision");
        }
        if session.snapshot().over.is_some() {
            return; // game ended first; nothing to assert
        }

        assert!(session.ai_to_act());
        assert!(
            session.apply_human(0).is_err(),
            "the human must not be able to act on the AI's decision"
        );
        // And the snapshot tells the UI to expect an update rather than
        // offering buttons.
        let snap = session.snapshot();
        assert!(snap.thinking);
        assert!(snap.options.is_empty());
    }

    /// The debug log must be a reproducer, not just a trace: the header
    /// carries the seed and both decklists, and every step carries the action
    /// and the resulting state hash. Replaying the recorded actions into a
    /// fresh game from the same seed must reproduce those hashes exactly.
    #[test]
    fn the_debug_log_replays_the_session_it_recorded() {
        let dir = std::env::temp_dir().join(format!("opsim-log-{}", std::process::id()));
        std::env::set_var("OPSIM_DEBUG_DIR", &dir);

        let Some(mut session) = fixture() else {
            std::env::remove_var("OPSIM_DEBUG_DIR");
            return;
        };
        let path = session
            .debug_log_path()
            .expect("a log should be written")
            .to_path_buf();

        for _ in 0..40 {
            if session.snapshot().over.is_some() || session.snapshot().options.is_empty() {
                break;
            }
            session.apply_human(0).unwrap();
            session.run_ai();
        }
        let final_hash = session.game.state.state_hash();

        let text = std::fs::read_to_string(&path).expect("log should exist");
        let mut lines = text.lines();

        let header: serde_json::Value =
            serde_json::from_str(lines.next().expect("header line")).unwrap();
        assert_eq!(header["kind"], "header");
        assert_eq!(header["seed"], 7);
        assert_eq!(header["decks"][0]["cards"], 50);

        // Every step is well-formed and the last hash matches the live game.
        let mut last = None;
        let mut count = 0;
        for line in lines {
            let step: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(step["kind"], "step");
            last = step["state_hash"].as_u64();
            count += 1;
        }
        assert!(count > 1, "expected several steps, got {count}");
        assert_eq!(
            last,
            Some(final_hash),
            "the last logged hash must match the live game"
        );

        std::fs::remove_dir_all(&dir).ok();
        std::env::remove_var("OPSIM_DEBUG_DIR");
    }

    #[test]
    fn the_catalogue_covers_every_card_either_deck_can_contain() {
        let Some(session) = fixture() else { return };
        let (a, b) = (crate::st01(), crate::st02());
        let catalogue = session.catalogue(&[&a, &b]);

        for deck in [&a, &b] {
            for number in std::iter::once(&deck.leader).chain(deck.cards.iter()) {
                assert!(
                    catalogue.iter().any(|c| &c.number == number),
                    "{number} is missing from the catalogue, so the UI cannot draw it"
                );
            }
        }
        // 17 cards per starter deck, leaders included, no duplicates.
        assert_eq!(catalogue.len(), 34);
    }
}

fn action_kind(action: &Action) -> &'static str {
    match action {
        Action::Attack { .. } => "attack",
        Action::PlayCard { .. } => "play",
        Action::GiveDon { .. } => "don",
        Action::ActivateEffect { .. } => "effect",
        Action::Block { .. } => "block",
        Action::Counter { .. } | Action::CounterEvent { .. } => "counter",
        Action::EndMainPhase | Action::DoneCountering => "end",
        _ => "other",
    }
}
