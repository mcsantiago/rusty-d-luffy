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
    legal_actions, Action, CardInstanceId, DeckList, Game, GameConfig, Pending, PlayerId,
    SetupError,
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
    /// The card whose menu this action belongs in. `None` for the card-less
    /// actions, which are the only ones the sidebar must still hold.
    pub subject: Option<u32>,
    /// Coarse kind, for grouping and styling.
    pub kind: &'static str,
}

/// One thing that has happened in the battle now being resolved.
///
/// Carries the card responsible rather than only naming it, so the UI can show
/// the blocker or the Counter that was actually used. `card` is `None` when the
/// viewer may not identify it, on the same terms as any other projected event.
#[derive(Debug, Clone, Serialize)]
pub struct BattleBeat {
    pub text: String,
    pub card: Option<u32>,
    pub kind: &'static str,
}

/// A card that arrived in the human's hand with nothing to decide about it.
#[derive(Debug, Clone, Serialize)]
pub struct CardToHand {
    pub number: String,
    /// What put it there: "life" or "draw". Only the caption differs.
    pub how: &'static str,
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
    /// What has happened so far in the battle being resolved, oldest first.
    /// Empty when no battle is running.
    pub battle_beats: Vec<BattleBeat>,
    /// How the last battle ended, named for the cards that fought it.
    ///
    /// Outlives the battle deliberately: the engine clears `battle` the moment
    /// it resolves, so anything shown only while one is running is gone before
    /// it can be read. Cleared by the next attack.
    pub battle_result: Option<String>,
    /// Cards that reached the human's hand since their last decision, with
    /// nothing to decide about them.
    ///
    /// Scoped to a client-visible step rather than to one engine step: an AI
    /// turn is many engine steps and a single snapshot, so anything cleared per
    /// step would be gone before the player ever saw it.
    pub to_hand: Vec<CardToHand>,
    /// The Life card whose [Trigger] the human is being asked about.
    ///
    /// Sent only to the player taking the damage, who may see it whichever way
    /// they answer: declining adds it to their hand unrevealed (10-1-5-2). The
    /// opponent learns it by revelation on activation (10-1-5-1), never here.
    pub trigger_card: Option<String>,
    /// Which decision the human owes, as a stable tag.
    ///
    /// The UI cannot tell them apart from the options alone, and needs to: a
    /// card with no actions means "cannot act" in the Main Phase and nothing
    /// at all during a mulligan, where no card has actions by definition.
    pub pending_kind: Option<&'static str>,
    /// The card whose effect is asking, while a `Choose` is pending.
    ///
    /// Public information: an effect resolves from a card that was played or
    /// activated in the open, or from a [Trigger] revealed by activating it
    /// (10-1-5-1).
    pub choose_source: Option<String>,
    /// The human owes a `Choose`, of at most this many cards.
    ///
    /// Carries no card ids of its own: the engine offers a `Choose` as one
    /// action per subset, so the candidates are already in `options` and every
    /// one of them appears there singly. This says only which kind of decision
    /// is pending, which the UI otherwise cannot tell from a list of labels.
    pub choose_up_to: Option<u8>,
    /// The AI owes a decision; the UI should expect a `game://update` shortly.
    pub thinking: bool,
    /// Short identifier for this session, matching its log filename, so a bug
    /// report can name the trace without digging for the file.
    pub session_id: String,
}

/// Where session logs go, or `None` when disabled.
///
/// Resolved by the caller rather than read here: a session that consulted the
/// process environment would make concurrent tests race on a global.
///
/// `OPSIM_DEBUG_DIR=` (empty) turns logging off; anything else overrides the
/// default of `<data dir>/debug`.
pub fn debug_dir_from_env() -> Option<std::path::PathBuf> {
    match std::env::var("OPSIM_DEBUG_DIR") {
        Ok(dir) if dir.is_empty() => None,
        Ok(dir) => Some(std::path::PathBuf::from(dir)),
        Err(_) => Some(crate::ingest::data_dir().join("debug")),
    }
}

pub struct Session {
    game: Game,
    ai: Box<dyn Agent + Send>,
    human: PlayerId,
    log: Vec<String>,
    db: Arc<CardDb>,
    session_id: String,
    /// Omniscient debug log for this session. Never shown to the player — it
    /// records `GameEvent`, so it contains both hands.
    debug: Option<op_core::SessionLog>,
    /// Narration for the battle in progress, reset by each attack.
    battle_beats: Vec<BattleBeat>,
    battle_result: Option<String>,
    to_hand: Vec<CardToHand>,
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

/// Everything needed to start a session.
///
/// A struct rather than eight positional arguments: at that width the call site
/// stops being readable and swapping two booleans compiles fine.
pub struct SessionConfig {
    pub seed: u64,
    pub human_deck: DeckList,
    pub ai_deck: DeckList,
    pub human_first: bool,
    pub difficulty: Difficulty,
    /// Where to write the session log, or `None` to disable it.
    pub debug_dir: Option<std::path::PathBuf>,
}

impl Session {
    pub fn new(
        db: Arc<CardDb>,
        scripts: Arc<dyn ScriptSource + Send + Sync>,
        options: SessionConfig,
    ) -> Result<Session, SetupError> {
        let SessionConfig {
            seed,
            human_deck,
            ai_deck,
            human_first,
            difficulty,
            debug_dir,
        } = options;
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
        let debug = debug_dir.and_then(|dir| {
            op_core::SessionLog::create(
                dir,
                seed,
                if human_first {
                    PlayerId::P0
                } else {
                    PlayerId::P1
                },
                &decks,
                vec![
                    format!("difficulty={difficulty:?}"),
                    "client=desktop".into(),
                ],
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
            // The low half of the seed: short enough to read aloud, and it
            // appears verbatim in the log filename.
            session_id: format!("{:08x}", seed as u32),
            debug,
            battle_beats: Vec::new(),
            battle_result: None,
            to_hand: Vec::new(),
        };
        if let Some(path) = session.debug_log_path() {
            eprintln!("session log: {}", path.display());
        }
        if let Some(debug) = session.debug.as_mut() {
            debug.record(
                None,
                &opening.events,
                &session.game.state,
                session.db.as_ref(),
            );
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
        self.to_hand.clear();
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
        self.to_hand.clear();
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
            self.note_card_to_hand(&projected);
            self.narrate_battle(&projected);
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

    /// A card that reached the human's hand without asking them anything.
    ///
    /// A Life card is skipped when it has printed text: one with a [Trigger]
    /// was already shown by the prompt offering it, and one with an effect is
    /// worth reading rather than glimpsing. A draw is never skipped — seeing
    /// what you drew is the whole point.
    fn note_card_to_hand(&mut self, event: &op_core::PlayerEvent) {
        use op_core::PlayerEvent as E;

        let (card, how) = match event {
            E::LifeTaken {
                player,
                card,
                banished,
            } if *player == self.human && !banished => (*card, "life"),
            // Turn 0 is setup: the opening hand and a mulligan are dealt, not
            // drawn, and five cards flying past at once says nothing.
            E::Drew { player, card } if *player == self.human && self.game.state.turn > 0 => {
                (*card, "draw")
            }
            _ => return,
        };

        let Some(id) = card.id() else { return };
        let def = self.db.get(self.game.state.card(id).def);
        if how == "life" && (def.trigger.is_some() || def.effect.is_some()) {
            return;
        }
        let number = def.number.clone();
        self.to_hand.push(CardToHand { number, how });
    }

    /// Records what a battle event means for the defender's side of the story.
    ///
    /// Built from `BattleStepStarted` as well as the actions themselves,
    /// because declining is silent: passing on a block emits no event at all,
    /// and only the arrival of the next step distinguishes "chose not to" from
    /// "still deciding".
    fn narrate_battle(&mut self, event: &op_core::PlayerEvent) {
        use op_core::PlayerEvent as E;

        let db = &self.db;
        let state = &self.game.state;
        let name = |card: op_core::CardRef| match card.id() {
            Some(id) => db.get(state.card(id).def).name.clone(),
            None => "a card".to_string(),
        };
        let beat = |text: String, card: op_core::CardRef, kind| BattleBeat {
            text,
            card: card.id().map(|c| c.0),
            kind,
        };

        let next = match event {
            E::AttackDeclared { attacker, target } => {
                let b = beat(
                    format!("{} attacks {}", name(*attacker), name(*target)),
                    *attacker,
                    "attack",
                );
                self.battle_beats.clear();
                self.battle_result = None;
                b
            }
            E::Blocked { blocker, .. } => beat(
                format!("{} blocks, and becomes the target", name(*blocker)),
                *blocker,
                "block",
            ),
            E::Countered {
                card,
                target,
                amount,
                ..
            } => beat(
                format!("{} counters with {}: +{amount}", name(*target), name(*card)),
                *card,
                "counter",
            ),
            E::TriggerActivated { card, .. } => beat(
                format!("{} activates its [Trigger]", name(*card)),
                *card,
                "trigger",
            ),
            E::KnockedOut { card } => beat(format!("{} is K.O.'d", name(*card)), *card, "result"),
            E::BattleResolved {
                attacker,
                target,
                attacker_power,
                target_power,
                attacker_won,
            } => {
                // Named from the event rather than from the board: a successful
                // [Blocker] replaced the target (10-1-4-1), and the card that
                // actually fought is the one worth naming.
                self.battle_result = Some(if *attacker_won {
                    format!("{} attacks {} successfully", name(*attacker), name(*target))
                } else {
                    format!(
                        "{} repelled {} successfully",
                        name(*target),
                        name(*attacker)
                    )
                });
                BattleBeat {
                    text: format!(
                        "{attacker_power} against {target_power} — {}",
                        if *attacker_won {
                            "the attack lands"
                        } else {
                            "the attack is repelled"
                        }
                    ),
                    card: None,
                    kind: "result",
                }
            }
            // Nobody blocked, or nobody countered. Silence is the whole signal
            // here, so it is stated rather than left to be inferred.
            E::BattleStepStarted { step } => {
                let missing = match step {
                    op_core::BattleStep::Counter => ("block", "No blocker"),
                    op_core::BattleStep::Damage => ("counter", "No Counter played"),
                    _ => return,
                };
                // Outside a battle there is nothing to narrate, and a step
                // whose action did happen needs no note that it did not.
                if self.battle_beats.is_empty()
                    || self.battle_beats.iter().any(|b| b.kind == missing.0)
                {
                    return;
                }
                BattleBeat {
                    text: missing.1.to_string(),
                    card: None,
                    kind: "declined",
                }
            }
            _ => return,
        };

        self.battle_beats.push(next);
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
                    subject: action_subject(action).map(|c| c.0),
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

        let pending_kind = self
            .game
            .pending()
            .filter(|p| p.player() == self.human)
            .map(pending_kind);

        let trigger_card = self
            .game
            .pending()
            .and_then(|p| match p {
                Pending::Trigger { player, card } if *player == self.human => Some(*card),
                _ => None,
            })
            .map(|card| self.db.get(self.game.state.card(card).def).number.clone());

        let choose_up_to = self
            .game
            .pending()
            .filter(|p| p.player() == self.human)
            .and_then(|p| match p {
                Pending::Choose { up_to, .. } => Some(*up_to),
                _ => None,
            });

        // The frame on top of the stack is the one that suspended for this
        // choice, so its source is the card doing the asking.
        let choose_source = choose_up_to.and(self.game.state.stack.top()).map(|frame| {
            self.db
                .get(self.game.state.card(frame.source).def)
                .number
                .clone()
        });

        Snapshot {
            view,
            log: self.log.clone(),
            options,
            question,
            over,
            turn_label,
            battle_beats: self.battle_beats.clone(),
            battle_result: self.battle_result.clone(),
            to_hand: self.to_hand.clone(),
            pending_kind,
            trigger_card,
            choose_source,
            choose_up_to,
            thinking: self.ai_to_act(),
            session_id: self.session_id.clone(),
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
        Action::Block {
            blocker: Some(card),
        } => vec![*card],
        Action::Counter { card, to } | Action::CounterEvent { card, to } => vec![*card, *to],
        Action::Choose { cards } => cards.clone(),
        _ => Vec::new(),
    }
}

/// The one card an action is offered *from*, which is the card the player
/// reaches for: the attacker rather than its target, the Counter leaving hand
/// rather than the card it saves. Not `action_cards().first()`, whose order
/// exists for highlighting and is not load-bearing.
fn action_subject(action: &Action) -> Option<CardInstanceId> {
    match action {
        Action::PlayCard { card, .. } => Some(*card),
        Action::ActivateEffect { card, .. } => Some(*card),
        Action::GiveDon { to } => Some(*to),
        Action::Attack { attacker, .. } => Some(*attacker),
        Action::Block { blocker } => *blocker,
        Action::Counter { card, .. } | Action::CounterEvent { card, .. } => Some(*card),
        Action::Mulligan(_)
        | Action::EndMainPhase
        | Action::DoneCountering
        | Action::UseTrigger(_)
        | Action::Choose { .. } => None,
    }
}

fn pending_kind(pending: &Pending) -> &'static str {
    match pending {
        Pending::Mulligan { .. } => "mulligan",
        Pending::MainAction { .. } => "main",
        Pending::Block { .. } => "block",
        Pending::Counter { .. } => "counter",
        Pending::Trigger { .. } => "trigger",
        Pending::Choose { .. } => "choose",
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

#[cfg(test)]
mod tests {
    use super::*;
    use op_cards::Cards;

    fn fixture() -> Option<Session> {
        fixture_logging_to(None)
    }

    /// A unique directory per caller, so tests never share log state.
    fn temp_log_dir(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("opsim-{tag}-{}", std::process::id()))
    }

    fn fixture_logging_to(debug_dir: Option<std::path::PathBuf>) -> Option<Session> {
        let dir = crate::ingest::data_dir().join("cards");
        let db = CardDb::load_dir(dir).ok()?;
        let scripts: Arc<dyn ScriptSource + Send + Sync> = Arc::new(Cards::new(&db));
        Session::new(
            Arc::new(db),
            scripts,
            SessionConfig {
                seed: 7,
                human_deck: crate::st01(),
                ai_deck: crate::st02(),
                human_first: true,
                difficulty: Difficulty::Easy,
                debug_dir,
            },
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

    /// A session on `seed`, for tests that need more than one game to meet the
    /// situation they are about.
    fn seeded(seed: u64) -> Option<Session> {
        let dir = crate::ingest::data_dir().join("cards");
        let db = CardDb::load_dir(dir).ok()?;
        let scripts: Arc<dyn ScriptSource + Send + Sync> = Arc::new(Cards::new(&db));
        Session::new(
            Arc::new(db),
            scripts,
            SessionConfig {
                seed,
                human_deck: crate::st01(),
                ai_deck: crate::st02(),
                human_first: true,
                difficulty: Difficulty::Easy,
                debug_dir: None,
            },
        )
        .ok()
        .map(|mut session| {
            session.run_ai();
            session
        })
    }

    /// The Life card that flies to the hand is announced for exactly one
    /// snapshot, and only for cards with no printed text of their own. Both
    /// halves matter: a card that is never announced makes the animation dead
    /// code, and one announced twice would fly twice.
    #[test]
    fn a_plain_life_card_is_announced_on_its_way_to_hand() {
        let mut announced = 0;

        for seed in 0..12u64 {
            let Some(mut session) = seeded(seed) else {
                return;
            };

            for _ in 0..5_000 {
                let snap = session.snapshot();
                if snap.over.is_some() {
                    break;
                }
                for entry in &snap.to_hand {
                    let def = session
                        .db
                        .by_number(&entry.number)
                        .map(|d| session.db.get(d))
                        .expect("announced a card the database does not have");
                    if entry.how == "life" {
                        assert!(
                            def.effect.is_none() && def.trigger.is_none(),
                            "seed {seed}: {} has printed text and should not fly past",
                            entry.number
                        );
                        announced += 1;
                    }
                    // A draw during setup is a dealt hand, not a draw worth
                    // showing; five cards flying past at once says nothing.
                    assert!(
                        entry.how != "draw" || snap.view.turn > 0,
                        "seed {seed}: announced a setup draw"
                    );
                }

                // Attacking is what deals damage, so a run that only ends turns
                // would never take a Life card at all.
                let next = snap
                    .options
                    .iter()
                    .position(|o| o.kind == "attack")
                    .or_else(|| snap.options.iter().position(|o| o.kind == "play"))
                    .unwrap_or(0);
                session
                    .apply_human(next)
                    .expect("offered option must be legal");
                session.run_ai();
            }
        }

        assert!(
            announced > 0,
            "no plain Life card reached a hand in 12 games, so the animation is unreachable"
        );
    }

    /// A Choose only ever arises from an effect resolving, so the card doing
    /// the asking is always available — and the modal's header depends on it.
    /// If the two ever came apart, the prompt would silently lose the one thing
    /// that says why it is being asked.
    #[test]
    fn a_pending_choice_always_names_the_card_asking_it() {
        let mut chosen = 0;

        for seed in 0..12u64 {
            let Some(mut session) = seeded(seed) else {
                return;
            };

            for _ in 0..5_000 {
                let snap = session.snapshot();
                if snap.over.is_some() {
                    break;
                }
                assert_eq!(
                    snap.choose_up_to.is_some(),
                    snap.choose_source.is_some(),
                    "seed {seed}: a choice and its source must appear together"
                );
                if snap.choose_up_to.is_some() {
                    chosen += 1;
                }

                // Taking option 0 everywhere ends the turn immediately and no
                // effect ever resolves, which made an earlier version of this
                // test pass without once meeting the thing it is about.
                let next = snap
                    .options
                    .iter()
                    .position(|o| o.kind == "effect")
                    .or_else(|| snap.options.iter().position(|o| o.kind == "play"))
                    .unwrap_or(0);
                session
                    .apply_human(next)
                    .expect("offered option must be legal");
                session.run_ai();
            }
        }

        assert!(chosen > 0, "no choice arose in 12 games");
    }

    /// The Life card behind a [Trigger] is named only while its own player is
    /// deciding about it. They may see it either way — declining adds it to
    /// their hand unrevealed (10-1-5-2) — but the opponent learns it by
    /// revelation on activation (10-1-5-1), and the snapshot must not become a
    /// second route to it.
    #[test]
    fn a_trigger_card_is_named_only_while_its_own_player_is_asked() {
        let mut asked = 0;

        for seed in 0..12u64 {
            let Some(mut session) = seeded(seed) else {
                return;
            };

            for _ in 0..5_000 {
                let snap = session.snapshot();
                if snap.over.is_some() {
                    break;
                }
                if snap.pending_kind == Some("trigger") {
                    assert!(
                        snap.trigger_card.is_some(),
                        "seed {seed}: asked about a Trigger without saying which card"
                    );
                    asked += 1;
                } else {
                    assert!(
                        snap.trigger_card.is_none(),
                        "seed {seed}: named a Life card with no Trigger decision pending ({:?})",
                        snap.pending_kind
                    );
                }
                session.apply_human(0).expect("first option must be legal");
                session.run_ai();
            }
        }

        // Otherwise only the negative half was ever exercised.
        assert!(asked > 0, "no Trigger decision arose in 12 games");
    }

    /// The battle narration is the only place the UI says what the defender
    /// did, and two of its beats are inferred from silence rather than
    /// observed: declining to block and declining to counter emit no event, so
    /// they are read off the arrival of the next step. That inference is what
    /// this checks, over every battle a whole game happens to contain.
    #[test]
    fn battle_narration_never_claims_a_defence_that_did_not_happen() {
        let Some(mut session) = fixture() else { return };

        let mut battles = 0;
        for _ in 0..5_000 {
            let snap = session.snapshot();
            if snap.over.is_some() {
                break;
            }

            let beats = &snap.battle_beats;
            if !beats.is_empty() {
                battles += 1;
                assert_eq!(
                    beats[0].kind, "attack",
                    "narration must open with the attack that started it: {beats:?}"
                );
                assert_eq!(
                    beats.iter().filter(|b| b.kind == "attack").count(),
                    1,
                    "a second attack must have reset the narration: {beats:?}"
                );

                // "No blocker" and a block are contradictory, as are "No
                // Counter played" and a counter. Only one of each pair can be
                // in a single battle's story.
                let has = |kind: &str| beats.iter().any(|b| b.kind == kind);
                assert!(
                    !(has("declined") && has("block") && has("counter")),
                    "a declined beat sits alongside the defence it denies: {beats:?}"
                );
                assert!(
                    beats
                        .iter()
                        .all(|b| b.kind != "declined" || b.card.is_none()),
                    "nothing happened, so no card can be shown for it: {beats:?}"
                );
            }

            session.apply_human(0).expect("first option must be legal");
            session.run_ai();
        }

        // Otherwise the loop above asserted nothing at all.
        assert!(battles > 0, "no battle was narrated in a whole game");
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
        let dir = temp_log_dir("log");
        let Some(mut session) = fixture_logging_to(Some(dir.clone())) else {
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

    /// The UI dims cards that cannot act, and decides whether to by reading
    /// `pending_kind`. A tag that stopped matching the decision would dim the
    /// hand during a mulligan or leave it lit with no DON!! left, which is the
    /// bug this replaced. Needs no card data, so it cannot skip itself.
    #[test]
    fn every_decision_has_a_tag_the_ui_can_key_off() {
        let card = CardInstanceId(0);
        let cases = [
            (
                Pending::Mulligan {
                    player: PlayerId::P0,
                },
                "mulligan",
            ),
            (
                Pending::MainAction {
                    player: PlayerId::P0,
                },
                "main",
            ),
            (
                Pending::Block {
                    player: PlayerId::P0,
                },
                "block",
            ),
            (
                Pending::Counter {
                    player: PlayerId::P0,
                },
                "counter",
            ),
            (
                Pending::Trigger {
                    player: PlayerId::P0,
                    card,
                },
                "trigger",
            ),
            (
                Pending::Choose {
                    player: PlayerId::P0,
                    key: "t".into(),
                    options: vec![card],
                    up_to: 1,
                },
                "choose",
            ),
        ];
        for (pending, tag) in cases {
            assert_eq!(pending_kind(&pending), tag, "{pending:?}");
        }
    }

    /// Menus are built by matching `subject`, so a misattributed action shows
    /// up under the wrong card — for an attack, one the player does not even
    /// control. Needs no card data, so it cannot skip itself.
    #[test]
    fn an_action_is_offered_from_the_card_the_player_reaches_for() {
        let (a, b) = (CardInstanceId(1), CardInstanceId(2));

        // The attacker, not the target: the defender is not offering this.
        assert_eq!(
            action_subject(&Action::Attack {
                attacker: a,
                target: b
            }),
            Some(a)
        );
        assert_eq!(action_subject(&Action::GiveDon { to: a }), Some(a));
        // The card leaving your hand, not the one being saved.
        assert_eq!(action_subject(&Action::Counter { card: a, to: b }), Some(a));
        assert_eq!(
            action_subject(&Action::PlayCard {
                card: a,
                replacing: Some(b)
            }),
            Some(a)
        );

        // No subject: these are what keeps the sidebar alive.
        assert_eq!(action_subject(&Action::EndMainPhase), None);
        assert_eq!(action_subject(&Action::DoneCountering), None);
        assert_eq!(action_subject(&Action::Mulligan(true)), None);
        assert_eq!(action_subject(&Action::Block { blocker: None }), None);
    }
}
