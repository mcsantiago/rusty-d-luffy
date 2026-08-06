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
}

pub struct Session {
    game: Game,
    ai: Box<dyn Agent + Send>,
    human: PlayerId,
    log: Vec<String>,
    db: Arc<CardDb>,
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
        let (game, opening) = Game::new(config, Arc::clone(&db), scripts)?;

        let mut session = Session {
            game,
            ai: difficulty.agent(seed ^ 0x5EED),
            human: PlayerId::P0,
            log: Vec::new(),
            db,
        };
        session.absorb(&opening.events);
        session.run_ai();
        Ok(session)
    }

    /// Applies the human's chosen option, then lets the AI play until it is the
    /// human's turn again.
    pub fn choose(&mut self, index: usize) -> Result<(), String> {
        let legal = legal_actions(&self.game);
        let action = legal
            .get(index)
            .cloned()
            .ok_or_else(|| format!("option {index} is out of range"))?;
        let outcome = self.game.step(action).map_err(|e| e.to_string())?;
        self.absorb(&outcome.events);
        self.run_ai();
        Ok(())
    }

    /// Steps the AI while the pending decision belongs to it.
    fn run_ai(&mut self) {
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
            let Ok(outcome) = self.game.step(action) else {
                return;
            };
            self.absorb(&outcome.events);
        }
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
            session.choose(0).expect("first option must be legal");
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
            session.choose(0).unwrap();
        }
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
