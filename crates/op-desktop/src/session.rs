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
    /// For an arrangement, how many of `cards` go on top; the rest go to the
    /// bottom. `None` for every other action, whose `cards` need no split.
    ///
    /// Without it `cards` is lossy: "A on top, B and C underneath" and "A and B
    /// on top, C underneath" are the same sequence cut in different places, and
    /// the UI has to name one of them exactly to submit it.
    pub split: Option<usize>,
    /// Coarse kind, for grouping and styling.
    pub kind: &'static str,
}

/// One card that may be picked while a card-picking decision is pending.
///
/// The grid draws from the board where it can, so the label is a fallback for
/// candidates the view has no card for: a DON!! given to a Character is not in
/// the cost area the view publishes, and a DON!! has no art to draw anyway
/// (#29). Sent for every candidate rather than only those, so the client never
/// has to work out which case it is in.
#[derive(Debug, Clone, Serialize)]
pub struct ChoiceCandidate {
    pub id: u32,
    pub label: String,
    /// Cards sharing a class are interchangeable, so a pick differing from an
    /// offered option only within a class is the same answer. See
    /// [`choice_class`].
    pub class: String,
}

/// A key for cards a decision treats as interchangeable, letting the client
/// canonicalise a pick the engine offered only one representative of. Anything
/// that is not a DON!! is its own class, so matching there stays exact.
fn choice_class(id: op_core::CardInstanceId, game: &Game) -> String {
    let def = game.db().get(game.state.card(id).def);
    if def.category != op_core::Category::Don {
        return id.0.to_string();
    }
    match game.don_class(id) {
        op_core::DonClass::Given(holder) => format!("given:{}", holder.0),
        op_core::DonClass::Active => "active".into(),
        op_core::DonClass::Rested => "rested".into(),
    }
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

/// A card that has just gone to a trash pile.
#[derive(Debug, Clone, Serialize)]
pub struct CardToTrash {
    pub number: String,
    /// Whose trash it landed in, so the UI knows which pile to send it to.
    pub yours: bool,
    /// Where it came from: "field", "hand" or "life".
    pub from: &'static str,
    /// Why it went, in the player's words. An effect that removes a card is
    /// the opponent's doing and looks arbitrary without the card that did it.
    pub cause: String,
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
    /// Cards that reached a trash pile since the last decision.
    ///
    /// Built from the events that name the card — a K.O., a Counter spent, a
    /// banished Life card. A card trashed to pay an activation cost is not
    /// here, because the engine emits no event naming it.
    pub to_trash: Vec<CardToTrash>,
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
    /// The human owes a card-picking decision, of at most this many cards.
    ///
    /// Carries no card ids of its own: the engine offers such a decision as one
    /// action per subset, so the candidates are already in `options`. This says
    /// only which kind of decision is pending, which the UI otherwise cannot
    /// tell from a list of labels.
    ///
    /// Covers `Choose` and the `DON!! −X` selection, which the modal renders
    /// the same way — a grid of candidates and a count.
    pub choose_up_to: Option<u8>,
    /// Fewest cards a legal answer may name, alongside `choose_up_to`.
    ///
    /// Zero for the usual "up to N" (8-4-4-1). Equal to `choose_up_to` where
    /// the count is fixed rather than offered: a mandatory trash, or a
    /// `DON!! −X` that takes exactly X (8-3-1-6). Without it the modal cannot
    /// tell "pick up to 2" from "pick 2", and would enable its confirm button
    /// on an answer the engine will reject.
    pub choose_at_least: Option<u8>,
    /// The cards a pending card-picking decision may take, in the engine's
    /// order. Empty when no such decision is pending.
    pub choose_candidates: Vec<ChoiceCandidate>,
    /// The human owes an arrangement: these cards, top-first as they came off
    /// the deck, each to be put back on the top or the bottom.
    ///
    /// Carried explicitly rather than derived from `options` the way
    /// `choose_up_to` is. Every option holds all of the cards — they differ
    /// only in the order and where the split falls — so there is no singleton
    /// to read the candidate list off, and the draw order would be lost.
    pub arrange: Option<Vec<u32>>,
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
    to_trash: Vec<CardToTrash>,
    /// The card whose effect is resolving, for attributing what it removes.
    /// Survives between engine steps: an [On Play] announces itself in one and
    /// K.O.s in the next, once the controller has chosen a target.
    resolving: Option<String>,
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
        // Cloned so the log is written from the same config the game ran, but
        // only after setup succeeds — a rejected decklist should not leave a
        // log behind for a game that never started.
        let logged = config.clone();
        let (game, opening) = Game::new(config, Arc::clone(&db), scripts)?;

        // Best-effort: a session that cannot write a debug log still plays.
        let debug = debug_dir.and_then(|dir| {
            op_core::SessionLog::create(
                dir,
                &logged,
                Some(&op_ingest::source_ref()),
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
            to_trash: Vec::new(),
            resolving: None,
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
        self.to_trash.clear();
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
        self.to_trash.clear();
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
            self.note_card_to_trash(&projected);
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

    /// A card that has just landed in a trash pile.
    ///
    /// Only the events that name a card can be used, which leaves one real
    /// gap: trashing a card to pay an activation cost — ST02-001's leader and
    /// several ST-06 cards — emits nothing naming it, so it cannot be shown.
    fn note_card_to_trash(&mut self, event: &op_core::PlayerEvent) {
        use op_core::PlayerEvent as E;

        // An effect names itself when it activates and removes a card later,
        // once its controller has chosen one — an [On Play] announces itself in
        // one engine step and K.O.s in the next. A battle starting ends the
        // attribution: what a battle K.O.s, the battle K.O.'d.
        match event {
            E::EffectActivated { source, .. } | E::TriggerActivated { card: source, .. } => {
                self.resolving = source
                    .id()
                    .map(|id| self.db.get(self.game.state.card(id).def).name.clone());
            }
            E::AttackDeclared { .. } => self.resolving = None,
            _ => {}
        }

        let (card, from, cause) = match event {
            E::KnockedOut { card } => (
                *card,
                "field",
                match &self.resolving {
                    Some(by) => format!("K.O.'d by {by}"),
                    None => "K.O.'d in battle".to_string(),
                },
            ),
            E::Countered { card, .. } => (*card, "hand", "played as a Counter".to_string()),
            E::LifeTaken {
                card,
                banished: true,
                ..
            } => (*card, "life", "banished from Life".to_string()),
            _ => return,
        };

        // Trash is an open area (3-5-2), so a card that reached one is public
        // and the projection will have named it.
        let Some(id) = card.id() else { return };
        let instance = self.game.state.card(id);
        let yours = instance.owner == self.human;
        let number = self.db.get(instance.def).number.clone();
        self.to_trash.push(CardToTrash {
            number,
            yours,
            from,
            cause,
        });
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
                    split: action_split(action),
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

        let mine = self.game.pending().filter(|p| p.player() == self.human);
        let (choose_up_to, choose_at_least) = mine
            .and_then(|p| match p {
                Pending::Choose {
                    up_to, at_least, ..
                } => Some((*up_to, *at_least)),
                // A `DON!! −X` takes exactly X (8-3-1-6), so the floor and the
                // ceiling are the same number and the modal becomes "pick X".
                Pending::ReturnDon { n, .. } => Some((*n, *n)),
                _ => None,
            })
            .unzip();

        // The card doing the asking. A `ReturnDon` carries its own source,
        // because the cost may belong to an Event with no ops — nothing is on
        // the stack to read it from. A `Choose` is always asked from the frame
        // that suspended for it, which is the one on top.
        let choose_source = match mine {
            Some(Pending::ReturnDon { source, .. }) => Some(*source),
            _ => choose_up_to
                .and(self.game.state.stack.top())
                .map(|f| f.source),
        }
        .map(|card| self.db.get(self.game.state.card(card).def).number.clone());

        // Every card the decision may take. Read from the pending itself where
        // it publishes one: a `ReturnDon` offers one representative per class,
        // so the union of the offered sets is a strict subset of the pool and
        // would hide DON!! the player may legally pick.
        let mut choose_candidates: Vec<ChoiceCandidate> = Vec::new();
        if choose_up_to.is_some() {
            let pool: Vec<u32> = match mine {
                Some(Pending::ReturnDon { options, .. }) => options.iter().map(|d| d.0).collect(),
                _ => options
                    .iter()
                    .flat_map(|choice| choice.cards.iter().copied())
                    .collect(),
            };
            for id in pool {
                if choose_candidates.iter().any(|c| c.id == id) {
                    continue;
                }
                let card = op_core::CardInstanceId(id);
                choose_candidates.push(ChoiceCandidate {
                    id,
                    label: crate::render::candidate_label(card, &self.game),
                    class: choice_class(card, &self.game),
                });
            }
        }
        // The frame on top of the stack is the one that suspended for this
        // choice, so its source is the card doing the asking.
        let arrange = self
            .game
            .pending()
            .filter(|p| p.player() == self.human)
            .and_then(|p| match p {
                Pending::Arrange { cards, .. } => Some(cards.iter().map(|c| c.0).collect()),
                _ => None,
            });

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
            to_trash: self.to_trash.clone(),
            pending_kind,
            trigger_card,
            choose_source,
            choose_up_to,
            choose_at_least,
            choose_candidates,
            arrange,
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
        Action::ReturnDon { dons } => dons.clone(),
        // Top first, then bottom. `Choice::split` says where the boundary is.
        Action::Arrange { top, bottom } => top.iter().chain(bottom).copied().collect(),
        _ => Vec::new(),
    }
}

/// How many of an action's cards belong to the first of two ordered groups.
fn action_split(action: &Action) -> Option<usize> {
    match action {
        Action::Arrange { top, .. } => Some(top.len()),
        _ => None,
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
        // The question belongs to the card whose effect is asking, but the
        // answer is a yes/no the sidebar owns rather than a card menu.
        Action::PayCost(_) => None,
        Action::Mulligan(_)
        | Action::EndMainPhase
        | Action::DoneCountering
        | Action::UseTrigger(_)
        | Action::Choose { .. }
        // Answered in a modal over the cards themselves, not from a menu hung
        // off one card.
        | Action::Arrange { .. }
        | Action::ReturnDon { .. } => None,
    }
}

fn pending_kind(pending: &Pending) -> &'static str {
    match pending {
        Pending::Mulligan { .. } => "mulligan",
        Pending::MainAction { .. } => "main",
        Pending::Block { .. } => "block",
        Pending::Counter { .. } => "counter",
        Pending::Trigger { .. } => "trigger",
        Pending::PayCost { .. } => "pay-cost",
        Pending::ReturnDon { .. } => "return-don",
        Pending::Arrange { .. } => "arrange",
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
        // Separate kinds: ending your turn hands the game over and is worth
        // flagging, while finishing the Counter step is routine.
        Action::EndMainPhase => "end-turn",
        Action::DoneCountering => "done",
        // The opening decision is between two answers, not a list, so each is
        // coloured for what it does rather than sharing one neutral style.
        Action::Mulligan(false) => "keep",
        Action::Mulligan(true) => "mulligan",
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
                human_deck: op_cards::decks::st01(),
                ai_deck: op_cards::decks::st02(),
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
                human_deck: op_cards::decks::st01(),
                ai_deck: op_cards::decks::st02(),
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

    /// Cards reaching a trash pile are announced so the UI can animate them.
    ///
    /// The gap this documents is as important as the coverage: trashing a card
    /// to pay an activation cost emits no event naming it, so it cannot appear
    /// here. Everything that *is* announced comes from an event that names the
    /// card, and lands in the pile belonging to its owner.
    #[test]
    fn cards_reaching_a_trash_are_announced_with_where_they_came_from() {
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
                for entry in &snap.to_trash {
                    assert!(
                        matches!(entry.from, "field" | "hand" | "life"),
                        "seed {seed}: unknown origin {:?}",
                        entry.from
                    );
                    assert!(
                        session.db.by_number(&entry.number).is_some(),
                        "seed {seed}: {} is not in the database",
                        entry.number
                    );
                    announced += 1;
                }

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

        assert!(announced > 0, "nothing reached a trash in 12 games");
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

    /// The debug log must be a reproducer, not just a trace.
    ///
    /// This drives a real session with real card data, then rebuilds the game
    /// from the log alone — config from the header, actions from the steps —
    /// and requires it to land on the live game's state hash. Parsing the file
    /// and comparing it to the session that wrote it would prove only that
    /// serialisation round-trips; the claim being pinned here is that the
    /// recorded config and actions are *sufficient* to reconstruct the game.
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

        let record = op_core::replay::read(&path).expect("the log should parse");
        assert!(!record.truncated, "a cleanly written log is not truncated");
        assert_eq!(record.header.seed, 7);
        assert_eq!(record.header.decks[0].cards.len(), 50);
        assert!(record.steps.len() > 1, "expected several steps");
        // The card-data revision has to be in the file, or a divergent replay
        // cannot be told apart from a bumped pin.
        assert_eq!(
            record.header.card_data_ref.as_deref(),
            Some(op_ingest::source_ref().as_str())
        );

        // Rebuilt from the log, not borrowed from the session.
        let db = CardDb::load_dir(crate::ingest::data_dir().join("cards")).expect("card data");
        let scripts: Arc<dyn ScriptSource + Send + Sync> = Arc::new(Cards::new(&db));
        let verified = record
            .verify(Arc::new(db), scripts)
            .expect("the log should replay");

        assert_eq!(
            verified.final_hash, final_hash,
            "replaying the log must land on the live game's position"
        );
        assert_eq!(verified.steps, record.steps.len());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_catalogue_covers_every_card_either_deck_can_contain() {
        let Some(session) = fixture() else { return };
        let (a, b) = (op_cards::decks::st01(), op_cards::decks::st02());
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
                    at_least: 0,
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

    /// The contract the arrange modal is built on: an option's `cards` are the
    /// top pile followed by the bottom pile, and `split` says where the join
    /// is. The UI builds an arrangement and then looks for the option that is
    /// exactly it, so a pair that did not round-trip would leave a legal
    /// arrangement unsubmittable.
    #[test]
    fn an_arrangement_round_trips_through_cards_and_split() {
        let ids: Vec<CardInstanceId> = (0..3).map(CardInstanceId).collect();

        for split in 0..=ids.len() {
            let top = ids[..split].to_vec();
            let bottom = ids[split..].to_vec();
            let action = Action::Arrange {
                top: top.clone(),
                bottom: bottom.clone(),
            };

            let cards = action_cards(&action);
            let at = action_split(&action).expect("an arrangement always splits");
            assert_eq!(at, top.len());
            assert_eq!(cards[..at], top[..], "the first {at} are the top pile");
            assert_eq!(cards[at..], bottom[..], "the rest are the bottom pile");
        }
    }

    /// Splitting is meaningless for everything else, and saying so keeps the UI
    /// from having to guess which actions carry two ordered groups.
    #[test]
    fn only_an_arrangement_reports_a_split() {
        let a = CardInstanceId(0);
        assert_eq!(action_split(&Action::EndMainPhase), None);
        assert_eq!(action_split(&Action::Choose { cards: vec![a] }), None);
        assert_eq!(action_split(&Action::GiveDon { to: a }), None);
    }
}
