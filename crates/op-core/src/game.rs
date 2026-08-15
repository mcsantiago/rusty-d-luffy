//! The engine: setup, the turn machine, the battle machine, and the step loop.

use std::sync::Arc;

use rand::seq::SliceRandom;

/// Something an activated effect needs, which the board may not currently hold.
///
/// Returned by [`Game::activation_shortfall`] so a client can name what is
/// missing. Presentation is the caller's: this says which pool came up empty,
/// not how to word it.
#[derive(Debug, Clone, Copy)]
pub enum Requirement<'a> {
    /// Cards matching this selector, for a `choose`.
    Cards(&'a crate::effect::Selector),
    /// DON!! in the controller's cost area that this source admits.
    Don(crate::effect::DonSource),
    /// A condition on the card that does not currently hold, which the effect
    /// would stop at. ST06-017's "if your Leader has the {Navy} type".
    Condition,
}

/// Whether a condition can be judged before the effect starts resolving.
///
/// `Condition::Bound` asks about a binding an earlier op makes — "you may X. If
/// you do, Y." — and the frame that has not run has none, so it would read
/// false for a condition that will hold. Undecidable is not false, and the
/// advice helpers pass over it rather than answering.
fn decidable(cond: &crate::effect::Condition) -> bool {
    !matches!(cond, crate::effect::Condition::Bound(_))
}

/// A requirement an activated effect does not have, and whether missing it
/// costs the whole effect.
#[derive(Debug, Clone, Copy)]
pub struct Shortfall<'a> {
    pub req: Requirement<'a>,
    /// Nothing else in the effect would happen anyway, so activating really is
    /// spending it for nothing.
    ///
    /// False where something has already run by the time the effect reaches
    /// this: ST06-015 draws a card and *then* looks for a Character to shrink,
    /// so an empty opponent board makes the second half idle and the draw is
    /// still worth having. A client that says "changes nothing" without this
    /// talks a player out of a free card.
    pub sole: bool,
}

/// The first choice an activated effect would ask, as [`Game::activation_choice`]
/// describes it before the activation is sent.
#[derive(Debug, Clone)]
pub struct ActivationChoice<'a> {
    pub select: &'a crate::effect::Selector,
    /// The cards it would offer. Empty when the pool is secret — see `secret`,
    /// which is what tells that apart from "nothing matches".
    pub options: Vec<CardInstanceId>,
    /// Whether the pool is a secret area (3-1-5). A client may say *that* a
    /// choice is coming, but not what is in it, and must not read anything
    /// into `options` being empty.
    pub secret: bool,
}

use crate::action::{Action, IllegalAction, Pending};
use crate::card::{CardDb, Category, Keyword};
use crate::derive::{self, Derived};
use crate::effect::{
    Duration, EffectFrame, ModKind, Modifier, PendingCost, Timing, COST_TRASH_KEY,
};
use crate::event::{GameEvent, PlayerEvent};
use crate::ids::{CardInstanceId, PlayerId};
use crate::script::{ScriptSource, BATTLED_BINDING};
use crate::state::{BattleState, BattleStep, DamageState, GameOver, GameState, Phase, Placement};
use crate::zone::Zone;

/// A deck as card numbers, e.g. leader `"ST01-001"` plus 50 others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeckList {
    pub leader: String,
    pub cards: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct GameConfig {
    pub seed: u64,
    pub first_player: PlayerId,
    pub decks: [DeckList; 2],
    /// Skip deck-construction validation. Useful for kernel tests that want a
    /// three-card deck.
    pub allow_illegal_decks: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SetupError {
    #[error("unknown card number {0}")]
    UnknownCard(String),
    #[error("card {0} is not a Leader")]
    NotALeader(String),
    #[error("deck must contain exactly 50 cards, found {0}")]
    DeckSize(usize),
    #[error("more than 4 copies of {0} in deck")]
    TooManyCopies(String),
}

/// What executing one effect op means for the frame's instruction pointer.
enum OpOutcome {
    /// Done; move to the next op.
    Advance,
    /// Waiting on a player. `ip` stays put so the op re-runs on resume.
    Suspend,
    /// Re-run this op immediately with updated bindings.
    Retry,
    /// Stop resolving this effect entirely (a failed "if" clause, 8-3-3).
    Abort,
}

/// The scratch binding key under which [`EffectOp::DigTop`] stashes the cards
/// it lifted off the deck, so the follow-up pass knows what to bottom.
fn dig_pool_key(key: &str) -> String {
    format!("{key}$pool")
}

/// The result of one [`Game::step`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepOutcome {
    /// Everything that happened, in order. Omniscient — project it with
    /// [`StepOutcome::for_player`] before sending it anywhere.
    pub events: Vec<GameEvent>,
    /// What the engine now needs, if the game is still running.
    pub pending: Option<Pending>,
}

impl StepOutcome {
    /// The events and pending decision as `viewer` may see them.
    ///
    /// A pending decision belonging to the opponent is withheld: which choice
    /// they are facing can itself be informative.
    pub fn for_player(&self, state: &GameState, viewer: PlayerId) -> PlayerOutcome {
        PlayerOutcome {
            events: self
                .events
                .iter()
                .map(|e| e.project(state, viewer))
                .collect(),
            pending: self.pending.clone().filter(|p| p.player() == viewer),
        }
    }
}

/// A [`StepOutcome`] redacted for one player. Safe to send to them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerOutcome {
    pub events: Vec<PlayerEvent>,
    pub pending: Option<Pending>,
}

/// Where a DON!! sits, which is the only thing telling two of them apart.
/// See [`Game::don_class`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DonClass {
    /// Rested in the cost area: it cannot be spent again this turn anyway.
    Rested,
    /// Given to a card (6-5-5-1), so it is on that holder, not in the cost area.
    Given(CardInstanceId),
    /// Active in the cost area, and so still spendable this turn.
    Active,
}

/// A game in progress.
///
/// The card database and scripts are immutable and shared; all mutable state is
/// in [`Game::state`], which is plain data.
pub struct Game {
    pub state: GameState,
    db: Arc<CardDb>,
    scripts: Arc<dyn ScriptSource + Send + Sync>,
}

impl Clone for Game {
    fn clone(&self) -> Game {
        Game {
            state: self.state.clone(),
            db: Arc::clone(&self.db),
            scripts: Arc::clone(&self.scripts),
        }
    }
}

impl Game {
    pub fn db(&self) -> &CardDb {
        &self.db
    }

    pub fn scripts(&self) -> &dyn ScriptSource {
        self.scripts.as_ref()
    }

    /// Current derived characteristics for every card.
    pub fn derived(&self) -> Derived {
        derive::derive_all(&self.state, &self.db, self.scripts.as_ref())
    }

    pub fn pending(&self) -> Option<&Pending> {
        self.state.pending.as_ref()
    }

    pub fn is_over(&self) -> bool {
        self.state.game_over.is_some()
    }

    pub fn result(&self) -> Option<GameOver> {
        self.state.game_over
    }

    /// Builds the opening position and runs setup up to the first decision
    /// (5-2-1).
    pub fn new(
        config: GameConfig,
        db: Arc<CardDb>,
        scripts: Arc<dyn ScriptSource + Send + Sync>,
    ) -> Result<(Game, StepOutcome), SetupError> {
        let mut state = GameState::new(config.seed, config.first_player);
        let mut events = vec![GameEvent::GameStarted {
            first_player: config.first_player,
        }];

        for (idx, deck) in config.decks.iter().enumerate() {
            let player = PlayerId(idx as u8);

            if !config.allow_illegal_decks {
                validate_deck(deck)?;
            }

            let leader_def = db
                .by_number(&deck.leader)
                .ok_or_else(|| SetupError::UnknownCard(deck.leader.clone()))?;
            if db.get(leader_def).category != Category::Leader {
                return Err(SetupError::NotALeader(deck.leader.clone()));
            }
            state.spawn(leader_def, player, Zone::Leader);

            for number in &deck.cards {
                let def = db
                    .by_number(number)
                    .ok_or_else(|| SetupError::UnknownCard(number.clone()))?;
                state.spawn(def, player, Zone::Deck);
            }

            // 10 DON!! (5-1-2).
            let don = db.don();
            for _ in 0..10 {
                state.spawn(don, player, Zone::DonDeck);
            }
        }

        // 5-2-1-2: shuffle. Both decks are shuffled from the one RNG stream, in
        // fixed player order, so the whole game stays a function of the seed.
        for idx in 0..2 {
            let mut deck = std::mem::take(&mut state.players[idx].deck);
            deck.shuffle(&mut state.rng);
            state.players[idx].deck = deck;
        }

        // 5-2-1-6: opening hands of 5.
        for idx in 0..2 {
            let player = PlayerId(idx as u8);
            for _ in 0..5 {
                if let Some(card) = draw_one(&mut state, player) {
                    events.push(GameEvent::Drew { player, card });
                }
            }
        }

        // The player going first decides on their mulligan first (5-2-1-6).
        state.pending = Some(Pending::Mulligan {
            player: config.first_player,
        });

        let game = Game { state, db, scripts };
        let pending = game.state.pending.clone();
        Ok((game, StepOutcome { events, pending }))
    }

    /// Applies one action and runs the engine forward to the next decision.
    pub fn step(&mut self, action: Action) -> Result<StepOutcome, IllegalAction> {
        if self.state.game_over.is_some() {
            return Err(IllegalAction::GameOver);
        }
        let pending = self
            .state
            .pending
            .clone()
            .ok_or(IllegalAction::NothingPending)?;

        let mut events = Vec::new();
        self.apply(pending, action, &mut events)?;
        self.advance(&mut events);

        Ok(StepOutcome {
            events,
            pending: self.state.pending.clone(),
        })
    }

    // ---- action application ------------------------------------------------

    fn apply(
        &mut self,
        pending: Pending,
        action: Action,
        events: &mut Vec<GameEvent>,
    ) -> Result<(), IllegalAction> {
        match (&pending, &action) {
            (Pending::Mulligan { player }, Action::Mulligan(take)) => {
                let player = *player;
                if *take {
                    self.mulligan(player, events);
                }
                self.state.player_mut(player).mulliganed = true;
                events.push(GameEvent::Mulliganed {
                    player,
                    took: *take,
                });
                self.state.pending = None;
                Ok(())
            }

            (Pending::Arrange { player, cards, key }, Action::Arrange { top, bottom }) => {
                let player = *player;
                let key = key.clone();
                // Every looked-at card, each placed exactly once. Rejected
                // rather than tolerated: a short answer would silently leave
                // cards in limbo, in no area at all.
                let mut named: Vec<CardInstanceId> =
                    top.iter().chain(bottom.iter()).copied().collect();
                let mut expected = cards.clone();
                named.sort();
                expected.sort();
                if named != expected {
                    return Err(IllegalAction::Illegal(
                        "an arrangement must place every card looked at, exactly once".into(),
                    ));
                }

                // Every rejection happens before anything moves: an `Err` from
                // `step` has to leave the state as it found it, or a client
                // that retries is playing on from a board half-changed by the
                // attempt that failed.
                if self.state.resolution.current().is_none() {
                    return Err(IllegalAction::Illegal(
                        "no effect is waiting on an arrangement".into(),
                    ));
                }

                // Both lists read top-to-bottom within their group. The bottom
                // goes back in order, so its first entry sits above the rest;
                // the top goes back in reverse, so its first entry ends up
                // topmost once the later pushes are underneath it.
                for &card in bottom {
                    let owner = self.state.card(card).owner;
                    self.move_card(card, owner, Zone::Deck, Placement::Bottom, events);
                }
                for &card in top.iter().rev() {
                    let owner = self.state.card(card).owner;
                    self.move_card(card, owner, Zone::Deck, Placement::Top, events);
                }

                self.bind_current(&key, Vec::new());
                self.state.pending = None;
                let _ = player;
                Ok(())
            }

            (Pending::PayCost { player, .. }, Action::PayCost(pay)) => {
                let player = *player;
                let Some(current) = self.state.resolution.current_mut() else {
                    return Err(IllegalAction::Illegal(
                        "no effect is waiting on a cost".into(),
                    ));
                };
                // Left on the frame until it is actually paid: a hand cost asks
                // which cards to trash first, and that answer arrives as its own
                // action.
                let Some(pc) = current.pending_cost.clone() else {
                    return Err(IllegalAction::Illegal(
                        "that effect is not waiting on a cost".into(),
                    ));
                };
                let source = current.source;
                if !pay {
                    // 8-3-1-4: refused, so the effect never activates.
                    self.state.resolution.retire();
                    self.state.pending = None;
                    return Ok(());
                }
                // An earlier effect in the same window may have spent it.
                if !self.can_pay(player, source, &pc.cost) {
                    self.state.resolution.retire();
                    self.state.pending = None;
                    return Ok(());
                }
                // 10-2-14-1: a hand cost trashes cards *selected from* the hand,
                // and 8-4-4 leaves the selection to the player.
                let hand = self.state.player(player).hand.to_vec();
                let n = pc.cost.trash_from_hand as usize;
                if n > 0 && hand.len() > n {
                    self.state.pending = Some(Pending::Choose {
                        player,
                        key: COST_TRASH_KEY.to_string(),
                        options: hand,
                        up_to: n as u8,
                        at_least: n as u8,
                    });
                    return Ok(());
                }
                // One legal answer is not a decision, as in `Op::Choose`: a hand
                // no bigger than the cost has only one way to pay it.
                let discard = hand[..n].to_vec();
                self.settle_cost(player, source, &pc, &discard, events);
                Ok(())
            }

            (
                Pending::ReturnDon {
                    player, n, options, ..
                },
                Action::ReturnDon { dons },
            ) => {
                let (player, n) = (*player, *n as usize);
                if dons.len() != n {
                    return Err(IllegalAction::Illegal(format!(
                        "this costs {n} DON!! card(s), {} named",
                        dons.len()
                    )));
                }
                if let Some(bad) = dons.iter().find(|d| !options.contains(d)) {
                    return Err(IllegalAction::Illegal(format!(
                        "{bad:?} is not a DON!! this cost may take"
                    )));
                }
                // Naming the same DON!! twice would otherwise pay the cost once
                // and report it paid in full.
                if dons
                    .iter()
                    .enumerate()
                    .any(|(i, d)| dons[i + 1..].contains(d))
                {
                    return Err(IllegalAction::Illegal(
                        "the same DON!! card named twice".into(),
                    ));
                }
                let dons = dons.clone();
                self.return_don(player, &dons, events);
                self.state.pending = None;
                Ok(())
            }

            (Pending::MainAction { player }, _) => self.main_action(*player, action, events),

            (Pending::Block { player }, Action::Block { blocker }) => {
                self.resolve_block(*player, *blocker, events)
            }

            (Pending::Counter { player }, Action::Counter { card, to }) => {
                self.resolve_counter(*player, *card, *to, events)
            }
            (Pending::Counter { player }, Action::CounterEvent { card, to }) => {
                self.resolve_counter_event(*player, *card, *to, events)
            }
            (Pending::Counter { .. }, Action::DoneCountering) => {
                self.state.pending = None;
                self.enter_battle_step(BattleStep::Damage, events);
                Ok(())
            }

            (Pending::Trigger { player, card }, Action::UseTrigger(use_it)) => {
                let (player, card) = (*player, *card);
                self.resolve_trigger(player, card, *use_it, events);
                Ok(())
            }

            (
                Pending::Choose {
                    player,
                    key,
                    options,
                    up_to,
                    at_least,
                },
                Action::Choose { cards },
            ) => {
                if cards.len() > *up_to as usize {
                    return Err(IllegalAction::Illegal(format!(
                        "chose {} cards, at most {up_to} allowed",
                        cards.len()
                    )));
                }
                let floor = (*at_least as usize).min(options.len());
                if cards.len() < floor {
                    return Err(IllegalAction::Illegal(format!(
                        "chose {} cards, at least {floor} required",
                        cards.len()
                    )));
                }
                let mut seen = cards.clone();
                seen.sort();
                seen.dedup();
                if seen.len() != cards.len() {
                    return Err(IllegalAction::Illegal("named a card twice".into()));
                }
                if let Some(bad) = cards.iter().find(|c| !options.contains(c)) {
                    return Err(IllegalAction::Illegal(format!(
                        "{bad:?} is not a legal choice"
                    )));
                }
                let key = key.clone();
                let cards = cards.clone();
                // A cost's own question, not a script's: a frame runs no ops
                // while its cost is unpaid, so no `Choose` op can be waiting.
                if self
                    .state
                    .resolution
                    .current()
                    .is_some_and(|f| f.pending_cost.is_some())
                {
                    return self.pay_hand_cost(*player, &cards, events);
                }
                let _ = player;
                if let Some(frame) = self.state.resolution.current_mut() {
                    frame.bind(&key, cards);
                }
                self.state.pending = None;
                Ok(())
            }

            _ => Err(IllegalAction::WrongKind {
                action: format!("{action:?}"),
                pending: format!("{pending:?}"),
            }),
        }
    }

    fn main_action(
        &mut self,
        player: PlayerId,
        action: Action,
        events: &mut Vec<GameEvent>,
    ) -> Result<(), IllegalAction> {
        match action {
            Action::EndMainPhase => {
                self.state.pending = None;
                self.state.phase = Phase::End;
                // The other four phases are announced in `tick_phase`; the End
                // Phase is entered from here because the Main Phase ends on the
                // turn player's word, and a trace that omits the announcement
                // cannot tell a quiet End Phase from one that never ran.
                events.push(GameEvent::PhaseStarted {
                    phase: Phase::End,
                    player,
                });
                Ok(())
            }

            Action::GiveDon { to } => {
                let target = self.state.card(to);
                if target.controller != player
                    || !matches!(target.zone, Zone::Leader | Zone::Character)
                {
                    return Err(IllegalAction::Illegal(
                        "DON!! can only be given to your own Leader or Characters".into(),
                    ));
                }
                let don =
                    self.active_don(player).first().copied().ok_or_else(|| {
                        IllegalAction::Illegal("no active DON!! available".into())
                    })?;
                self.state.lift(don);
                self.state.card_mut(don).zone = Zone::Cost;
                self.state.card_mut(to).attached_don.push(don);
                events.push(GameEvent::DonGiven { player, don, to });
                self.state.pending = None;
                Ok(())
            }

            Action::PlayCard { card, replacing } => self.play_card(player, card, replacing, events),

            Action::ActivateEffect {
                card,
                slot,
                discard,
            } => self.activate_effect(player, card, slot, &discard, events),

            Action::Attack { attacker, target } => {
                self.declare_attack(player, attacker, target, events)
            }

            other => Err(IllegalAction::WrongKind {
                action: format!("{other:?}"),
                pending: "MainAction".into(),
            }),
        }
    }

    fn play_card(
        &mut self,
        player: PlayerId,
        card: CardInstanceId,
        replacing: Option<CardInstanceId>,
        events: &mut Vec<GameEvent>,
    ) -> Result<(), IllegalAction> {
        if self.state.card(card).zone != Zone::Hand || self.state.card(card).controller != player {
            return Err(IllegalAction::Illegal("card is not in your hand".into()));
        }
        let def = self.db.get(self.state.card(card).def);
        let (category, cost) = (def.category, def.cost);

        let available = self.active_don(player);
        if available.len() < cost as usize {
            return Err(IllegalAction::Illegal(format!(
                "need {cost} active DON!!, have {}",
                available.len()
            )));
        }

        // 3-7-6-1: with 5 Characters out, playing a sixth means trashing one of
        // them first. Which one is the player's choice, and the trash happens
        // *before* the new card arrives — so its [On Play] sees a board of four
        // plus itself.
        let full = self.state.player(player).characters.len() >= 5;
        match (category, full, replacing) {
            (Category::Character, true, None) => {
                return Err(IllegalAction::Illegal(
                    "the Character area is full; name a Character to trash (3-7-6-1)".into(),
                ))
            }
            (Category::Character, true, Some(victim)) => {
                if !self.state.player(player).characters.contains(&victim) {
                    return Err(IllegalAction::Illegal(
                        "can only trash one of your own Characters to make room".into(),
                    ));
                }
            }
            (_, _, Some(_)) => {
                return Err(IllegalAction::Illegal(
                    "nothing needs trashing to play that card".into(),
                ))
            }
            _ => {}
        }

        for don in available.into_iter().take(cost as usize) {
            self.state.card_mut(don).rested = true;
        }

        // Ordered per 3-7-6-1: room is made before the card is played.
        if let Some(victim) = replacing {
            self.move_card(victim, player, Zone::Trash, Placement::Top, events);
            // Not `KnockedOut`: 3-7-6-1-1 makes this rule processing, and
            // `Timing::OnCharacterKoed` is deliberately never queued for it.
            // Logged as a K.O. it reads as a trigger the engine dropped.
            events.push(GameEvent::CardMoved {
                card: victim,
                from: Zone::Character,
                to: Zone::Trash,
            });
        }

        match category {
            Category::Character => {
                self.move_card(card, player, Zone::Character, Placement::Bottom, events);
                self.state.card_mut(card).played_on_turn = Some(self.state.turn);
                events.push(GameEvent::CardPlayed {
                    player,
                    card,
                    cost_paid: cost,
                });
                // 10-2-6: [On Play] activates when the card is played.
                self.queue_autos(Timing::OnPlay, card, events);
            }
            Category::Stage => {
                // 3-8-5-1: the second Stage trashes the first. `CardPlayed`
                // names only the arriving card, so without this the one it
                // replaced leaves the field unmentioned.
                if let Some(existing) = self.state.player(player).stage {
                    self.move_card(existing, player, Zone::Trash, Placement::Top, events);
                    events.push(GameEvent::CardMoved {
                        card: existing,
                        from: Zone::Stage,
                        to: Zone::Trash,
                    });
                }
                self.move_card(card, player, Zone::Stage, Placement::Bottom, events);
                events.push(GameEvent::CardPlayed {
                    player,
                    card,
                    cost_paid: cost,
                });
                self.queue_autos(Timing::OnPlay, card, events);
            }
            Category::Event => {
                // 8-4-2: the Event is trashed, then its effect is carried out.
                // The [Main] effect is stored as the card's first activated
                // effect. Its printed cost is the DON!! already spent above,
                // but the text may name a further cost of its own — ST08-014
                // asks for a Life card.
                let main = self
                    .scripts
                    .script(self.state.card(card).def)
                    .activated
                    .first()
                    .cloned();
                let extra = main.as_ref().map(|a| a.cost.clone()).unwrap_or_default();

                // The trash comes first, so the log reads in the order 8-4-2
                // gives and the card is out of the hand before its own cost
                // looks there — a "trash 1 card from your hand" cost must not
                // be able to pay itself with the Event being played.
                self.move_card(card, player, Zone::Trash, Placement::Top, events);
                events.push(GameEvent::CardPlayed {
                    player,
                    card,
                    cost_paid: cost,
                });

                // 8-3-1-3: a cost that cannot be paid in full cannot be paid at
                // all, and then the effect does not resolve. The card is still
                // played and trashed.
                // An `activated[0]` that resolves to nothing is not an effect to
                // activate, and asking for its cost first would be asking a
                // player to pay for nothing.
                if let Some(a) = main.filter(|a| !a.ops.is_empty()) {
                    if extra.is_free() {
                        events.push(GameEvent::EffectActivated {
                            source: card,
                            controller: player,
                        });
                        self.state
                            .resolution
                            .push(EffectFrame::new(card, player, a.ops));
                    } else if self.can_pay(player, card, &extra) {
                        // 8-3-1-4: a "You may ...:" cost is the controller's to
                        // decline, and the DON!! already spent do not make the
                        // choice for them — ST08-014 asks for a Life card, which
                        // at 1 Life is worth more than the effect. Pushed unpaid,
                        // exactly as an auto effect's cost is; the frame runs no
                        // ops and announces itself only once paid.
                        let mut frame = EffectFrame::new(card, player, a.ops);
                        frame.pending_cost = Some(PendingCost {
                            cost: extra,
                            slot: a.slot,
                            once_per_turn: a.once_per_turn,
                        });
                        self.state.resolution.push(frame);
                    }
                }
            }
            Category::Leader | Category::Don => {
                return Err(IllegalAction::Illegal("that card cannot be played".into()))
            }
        }

        // Nothing here asks a question of its own any more: an Event's further
        // cost is carried on the frame and asked for by `resolve_current_frame`,
        // which is also where a `DON!! −X` selection inside one now surfaces.
        self.state.pending = None;
        Ok(())
    }

    /// Activates an `[Activate: Main]` effect (6-5-4-1, 8-4-1).
    fn activate_effect(
        &mut self,
        player: PlayerId,
        card: CardInstanceId,
        slot: u8,
        discard: &[CardInstanceId],
        events: &mut Vec<GameEvent>,
    ) -> Result<(), IllegalAction> {
        if self.state.card(card).controller != player || !self.state.card(card).zone.is_field() {
            return Err(IllegalAction::Illegal("card is not on your field".into()));
        }
        let effect = self
            .scripts
            .script(self.state.card(card).def)
            .activated
            .iter()
            .find(|a| a.slot == slot)
            .cloned()
            .ok_or_else(|| IllegalAction::Illegal("no such effect".into()))?;

        // 10-2-13: [Once Per Turn].
        if effect.once_per_turn && self.state.card(card).used_once_per_turn.contains(&slot) {
            return Err(IllegalAction::Illegal(
                "that effect has already been used this turn".into(),
            ));
        }
        // 8-4-1-1: conditions must be met before activation.
        if !effect.conditions.is_empty()
            && !derive::conditions_hold(
                &self.state,
                &self.db,
                &self.derived(),
                card,
                None,
                &effect.conditions,
            )
        {
            return Err(IllegalAction::Illegal(
                "the effect's conditions are not met".into(),
            ));
        }
        // 8-3-1-3: an activation cost that cannot be paid in full cannot be
        // paid at all, so it is checked before anything is spent.
        if !self.can_pay(player, card, &effect.cost) {
            return Err(IllegalAction::Illegal(
                "cannot pay the activation cost".into(),
            ));
        }
        self.check_hand_cost(player, effect.cost.trash_from_hand, discard)?;
        let owed = self.pay(player, card, &effect.cost, discard, events);

        if effect.once_per_turn {
            self.state.card_mut(card).used_once_per_turn.push(slot);
        }
        events.push(GameEvent::EffectActivated {
            source: card,
            controller: player,
        });
        self.state
            .resolution
            .push(EffectFrame::new(card, player, effect.ops));
        self.state.pending = owed;
        Ok(())
    }

    /// Checks the cards named for a "trash N cards from your hand" cost.
    ///
    /// The count comes from the cost and the cards from the live hand, so this
    /// holds for either way of paying: named up front with an `ActivateEffect`,
    /// or answered later to the `Choose` an auto effect's cost raises.
    fn check_hand_cost(
        &self,
        player: PlayerId,
        trash_from_hand: u8,
        discard: &[CardInstanceId],
    ) -> Result<(), IllegalAction> {
        if discard.len() != trash_from_hand as usize {
            return Err(IllegalAction::Illegal(format!(
                "this costs {} card(s) from hand, {} named",
                trash_from_hand,
                discard.len()
            )));
        }
        if let Some(bad) = discard
            .iter()
            .find(|c| !self.state.player(player).hand.contains(c))
        {
            return Err(IllegalAction::Illegal(format!(
                "{bad:?} is not in your hand"
            )));
        }
        // 8-3-1-3: naming the same card twice would pay a two-card cost with
        // one — the second trip to the trash finds it already there — and
        // report it paid in full.
        if discard
            .iter()
            .enumerate()
            .any(|(i, c)| discard[i + 1..].contains(c))
        {
            return Err(IllegalAction::Illegal("the same card named twice".into()));
        }
        Ok(())
    }

    /// Answers the `Choose` a hand cost raised (10-2-14-1), paying with the
    /// cards named. Validated against the hand rather than trusted, the same as
    /// the discard travelling with an `ActivateEffect`.
    fn pay_hand_cost(
        &mut self,
        player: PlayerId,
        discard: &[CardInstanceId],
        events: &mut Vec<GameEvent>,
    ) -> Result<(), IllegalAction> {
        let Some(current) = self.state.resolution.current() else {
            return Err(IllegalAction::Illegal(
                "no effect is waiting on a cost".into(),
            ));
        };
        let Some(pc) = current.pending_cost.clone() else {
            return Err(IllegalAction::Illegal(
                "that effect is not waiting on a cost".into(),
            ));
        };
        let source = current.source;
        self.check_hand_cost(player, pc.cost.trash_from_hand, discard)?;
        self.settle_cost(player, source, &pc, discard, events);
        Ok(())
    }

    /// Spends an agreed auto-effect cost and releases the frame to run.
    ///
    /// Clearing `pending_cost` is what releases it: while set, the frame runs no
    /// ops and `resolve_current_frame` re-asks for payment.
    fn settle_cost(
        &mut self,
        player: PlayerId,
        source: CardInstanceId,
        pc: &PendingCost,
        discard: &[CardInstanceId],
        events: &mut Vec<GameEvent>,
    ) {
        if let Some(current) = self.state.resolution.current_mut() {
            current.pending_cost = None;
        }
        let owed = self.pay(player, source, &pc.cost, discard, events);
        if pc.once_per_turn {
            self.state.card_mut(source).used_once_per_turn.push(pc.slot);
        }
        events.push(GameEvent::EffectActivated {
            source,
            controller: player,
        });
        self.state.pending = owed;
    }

    pub(crate) fn can_pay(
        &self,
        player: PlayerId,
        card: CardInstanceId,
        cost: &crate::script::ActivationCost,
    ) -> bool {
        if self.active_don(player).len() < cost.rest_don as usize {
            return false;
        }
        if cost.rest_self && self.state.card(card).rested {
            return false;
        }
        if self.state.player(player).hand.len() < cost.trash_from_hand as usize {
            return false;
        }
        if self.state.player(player).life.len() < cost.life_to_hand as usize {
            return false;
        }
        // Guarded: `returnable_don` allocates and sorts, and `can_pay` runs per
        // candidate card at every search node, where almost no cost has one.
        if cost.don_minus > 0 && self.returnable_don(player).len() < cost.don_minus as usize {
            return false;
        }
        true
    }

    /// What distinguishes one DON!! from another when a `DON!! −X` asks which
    /// to return. Two DON!! of the same class are interchangeable down to the
    /// state hash, so this is the whole substance of that decision — and the
    /// only equivalence a client, a label or a validator may rely on.
    pub fn don_class(&self, don: CardInstanceId) -> DonClass {
        match self.don_holder(don) {
            Some(holder) => DonClass::Given(holder),
            None if self.state.card(don).is_active() => DonClass::Active,
            None => DonClass::Rested,
        }
    }

    /// The card a DON!! has been given to, or `None` if it is loose in the cost
    /// area.
    ///
    /// Exists for clients naming a `DON!! −X` choice: one DON!! is much like
    /// another, so where it sits is the only thing that distinguishes the
    /// options, and a given DON!! is not in the cost area to be found.
    pub fn don_holder(&self, don: CardInstanceId) -> Option<CardInstanceId> {
        let owner = self.state.card(don).owner;
        self.state
            .battlers(owner)
            .into_iter()
            .find(|&c| self.state.card(c).attached_don.contains(&don))
    }

    /// Every DON!! a `DON!! −X` may take: 8-3-1-6 takes them "from their Leader
    /// area, Character area, and cost area", and a given DON!! (6-5-5-1) lives
    /// on its holder rather than in the cost area. Ordered rested-before-active
    /// so the leading answer surrenders what the player can least use.
    pub(crate) fn returnable_don(&self, player: PlayerId) -> Vec<CardInstanceId> {
        let mut pool: Vec<CardInstanceId> = self.state.player(player).cost_area.to_vec();
        pool.sort_by_key(|&d| self.state.card(d).is_active());
        for holder in self.state.battlers(player) {
            pool.extend(self.state.card(holder).attached_don.iter().copied());
        }
        pool
    }

    /// Pays everything a cost asks for except the `DON!! −X` selection, and
    /// reports the decision that selection still owes, if any.
    ///
    /// The selection is the player's (3-9-2) and so cannot be settled here.
    /// Callers park on the returned `Pending` instead of clearing `pending`,
    /// which holds the effect's frame — already pushed — until the answer
    /// arrives, since `advance` will not tick while a decision is outstanding.
    /// Nothing else can happen in between: the game is stopped on the question,
    /// so the only state that moves between activation and payment is the
    /// answer itself.
    #[must_use = "a DON!! −X selection left unasked is a cost never paid"]
    fn pay(
        &mut self,
        player: PlayerId,
        card: CardInstanceId,
        cost: &crate::script::ActivationCost,
        discard: &[CardInstanceId],
        events: &mut Vec<GameEvent>,
    ) -> Option<Pending> {
        // 8-3-1-5: one event per DON!! rested, since nothing else carries the
        // count — `EffectActivated` names only the source, so a `③` that rested
        // three and one that rested none would read alike.
        for don in self
            .active_don(player)
            .into_iter()
            .take(cost.rest_don as usize)
        {
            self.state.card_mut(don).rested = true;
            events.push(GameEvent::Rested { card: don });
        }
        if cost.rest_self {
            self.state.card_mut(card).rested = true;
            events.push(GameEvent::Rested { card });
        }
        // 10-2-14-1: the card is selected from the hand, so the log has to say
        // which. On the `Pending::PayCost` path the answer is a bare
        // `PayCost(true)` and names nothing.
        for &trashed in discard {
            let from = self.state.card(trashed).zone;
            self.move_card(trashed, player, Zone::Trash, Placement::Top, events);
            events.push(GameEvent::CardMoved {
                card: trashed,
                from,
                to: Zone::Trash,
            });
        }
        // Life cards come off the top and go to hand face-up. This is not
        // damage, so no `[Trigger]` activates (10-1-5 ties Triggers to damage).
        for _ in 0..cost.life_to_hand {
            let Some(&top) = self.state.player(player).life.first() else {
                break;
            };
            self.move_card(top, player, Zone::Hand, Placement::Top, events);
            events.push(GameEvent::LifeTaken {
                player,
                card: top,
                banished: false,
            });
        }
        if cost.don_minus > 0 {
            let pool = self.returnable_don(player);
            // `can_pay` ran first, so a short pool means the cost was checked
            // against a board that has since shrunk; take what there is rather
            // than deadlock.
            let n = (cost.don_minus as usize).min(pool.len());
            // Only ask when the pool holds DON!! that differ. Within one class
            // they are interchangeable down to the state hash, so a bigger pool
            // than the cost is not by itself a decision.
            let mixed = {
                let mut classes = pool
                    .iter()
                    .map(|&d| (self.don_holder(d), self.state.card(d).rested));
                let first = classes.next();
                classes.any(|c| Some(c) != first)
            };
            if pool.len() > n && mixed {
                return Some(Pending::ReturnDon {
                    player,
                    source: card,
                    n: n as u8,
                    options: pool,
                });
            }
            // One way to pay is not a decision.
            self.return_don(player, &pool[..n], events);
        }
        None
    }

    /// Sends the named DON!! back to the bottom of `player`'s DON!! deck.
    ///
    /// A given DON!! is not in the cost area — it was lifted out of it and
    /// lives on its holder — so it has to be detached as well as moved, or the
    /// holder keeps a stale id and goes on drawing +1000 power from it.
    fn return_don(
        &mut self,
        player: PlayerId,
        dons: &[CardInstanceId],
        events: &mut Vec<GameEvent>,
    ) {
        for &don in dons {
            for holder in self.state.battlers(player) {
                self.state
                    .card_mut(holder)
                    .attached_don
                    .retain(|&d| d != don);
            }
            self.state.lift(don);
            self.state.card_mut(don).rested = false;
            self.state
                .put(don, player, Zone::DonDeck, Placement::Bottom);
        }
        events.push(GameEvent::DonSpentToDonDeck {
            player,
            count: dons.len() as u8,
        });
    }

    // ---- battle ------------------------------------------------------------

    fn declare_attack(
        &mut self,
        player: PlayerId,
        attacker: CardInstanceId,
        target: CardInstanceId,
        events: &mut Vec<GameEvent>,
    ) -> Result<(), IllegalAction> {
        let derived = self.derived();
        if self.state.card(attacker).controller != player {
            return Err(IllegalAction::Illegal("not your card".into()));
        }
        if !derive::can_attack(&self.state, &self.db, &derived, attacker) {
            return Err(IllegalAction::Illegal("that card cannot attack".into()));
        }
        if !derive::attack_targets(&self.state, attacker).contains(&target) {
            return Err(IllegalAction::Illegal(
                "illegal attack target: only the opponent's Leader or a rested Character".into(),
            ));
        }

        // 7-1-1-1: declaring rests the attacker.
        self.state.card_mut(attacker).rested = true;
        events.push(GameEvent::Rested { card: attacker });

        self.state.battle = Some(BattleState {
            step: BattleStep::Attack,
            attacker,
            target,
            original_target: target,
            blocker_used: false,
            attacker_zone: self.state.card(attacker).zone,
            target_zone: self.state.card(target).zone,
        });
        events.push(GameEvent::AttackDeclared { attacker, target });
        events.push(GameEvent::BattleStepStarted {
            step: BattleStep::Attack,
        });
        self.state.pending = None;
        Ok(())
    }

    /// Runs the between-step check from 7-1-1-4 / 7-1-2-3 / 7-1-3-3: if either
    /// the attacker or the target has changed areas, skip straight to the end
    /// of the battle.
    fn battle_participants_intact(&self) -> bool {
        let Some(b) = &self.state.battle else {
            return false;
        };
        self.state.card(b.attacker).zone == b.attacker_zone
            && self.state.card(b.target).zone == b.target_zone
    }

    fn enter_battle_step(&mut self, step: BattleStep, events: &mut Vec<GameEvent>) {
        if let Some(b) = &mut self.state.battle {
            b.step = step;
            b.attacker_zone = self.state.cards[b.attacker.index()].zone;
            b.target_zone = self.state.cards[b.target.index()].zone;
        }
        events.push(GameEvent::BattleStepStarted { step });
    }

    fn resolve_block(
        &mut self,
        player: PlayerId,
        blocker: Option<CardInstanceId>,
        events: &mut Vec<GameEvent>,
    ) -> Result<(), IllegalAction> {
        let _ = player;
        self.state.pending = None;

        // Declining resolves nothing, so the Block Step is over here.
        let Some(blocker) = blocker else {
            self.enter_battle_step(BattleStep::Counter, events);
            return Ok(());
        };

        let legal = self.legal_blockers();
        if !legal.contains(&blocker) {
            return Err(IllegalAction::Illegal("that card cannot block".into()));
        }
        let old_target = self.state.battle.as_ref().unwrap().target;
        // 10-1-4-1: resting the blocker makes it the new target.
        self.state.card_mut(blocker).rested = true;
        events.push(GameEvent::Rested { card: blocker });
        let blocker_zone = self.state.card(blocker).zone;
        if let Some(b) = &mut self.state.battle {
            b.target = blocker;
            b.blocker_used = true;
            // Re-baselined with the target, or the between-step check would read
            // the blocker's zone against the zone of the card it replaced and
            // call an ordinary block a departure.
            b.target_zone = blocker_zone;
        }
        events.push(GameEvent::Blocked {
            blocker,
            replacing: old_target,
        });
        // 7-1-2-2 and 10-2-15-1: activating a [Blocker] is what fulfils
        // [On Block], so it fires here rather than from a battle step — the
        // Block Step is also entered when nobody blocks.
        self.queue_autos(Timing::OnBlock, blocker, events);
        // The step deliberately does not advance: the queued frames resolve
        // first, and `tick_battle` then applies 7-1-2-3 to what they did. Ending
        // the Block Step here would announce a Counter Step before knowing
        // whether the battle survives to have one.
        Ok(())
    }

    /// Characters the defending player may activate `[Blocker]` with (7-1-2-1,
    /// 10-1-4-1), after the attacker's blocking restrictions.
    pub fn legal_blockers(&self) -> Vec<CardInstanceId> {
        let Some(b) = &self.state.battle else {
            return Vec::new();
        };
        if b.blocker_used {
            return Vec::new();
        }
        let derived = self.derived();
        let attacker_ch = derived.get(b.attacker);
        if attacker_ch.cannot_be_blocked || attacker_ch.has_keyword(Keyword::Unblockable) {
            return Vec::new();
        }

        let defender = self.state.card(b.attacker).controller.opponent();
        self.state
            .player(defender)
            .characters
            .iter()
            .copied()
            .filter(|&c| {
                let card = self.state.card(c);
                // A blocker must be active to rest, and cannot already be the
                // card under attack.
                if card.rested || c == b.target {
                    return false;
                }
                if !derived.get(c).has_keyword(Keyword::Blocker) {
                    return false;
                }
                match attacker_ch.blocker_power_ceiling {
                    Some(limit) => derived.power(c) < limit,
                    None => true,
                }
            })
            .collect()
    }

    /// Cards in hand the defending player may trash for their Counter value
    /// (7-1-3-2-1), paired with the cards they could boost.
    pub fn legal_counters(&self) -> Vec<CardInstanceId> {
        let Some(b) = &self.state.battle else {
            return Vec::new();
        };
        let defender = self.state.card(b.attacker).controller.opponent();
        self.state
            .player(defender)
            .hand
            .iter()
            .copied()
            .filter(|&c| self.db.get(self.state.card(c).def).counter.is_some())
            .collect()
    }

    fn resolve_counter(
        &mut self,
        player: PlayerId,
        card: CardInstanceId,
        to: CardInstanceId,
        events: &mut Vec<GameEvent>,
    ) -> Result<(), IllegalAction> {
        if !self.legal_counters().contains(&card) {
            return Err(IllegalAction::Illegal(
                "that card has no Counter value or is not in your hand".into(),
            ));
        }
        // The boost may go to the Leader or any Character of the defender
        // (7-1-3-2-1), not only the card under attack.
        if self.state.card(to).controller != player
            || !matches!(self.state.card(to).zone, Zone::Leader | Zone::Character)
        {
            return Err(IllegalAction::Illegal(
                "Counter can only boost your own Leader or Character".into(),
            ));
        }
        let amount = self
            .db
            .get(self.state.card(card).def)
            .counter
            .expect("filtered to cards with a Counter value");

        self.move_card(card, player, Zone::Trash, Placement::Top, events);
        self.state.modifiers.push(Modifier {
            target: to,
            kind: ModKind::Power(amount),
            duration: Duration::ThisBattle,
            source: card,
            controller: player,
        });
        events.push(GameEvent::Countered {
            player,
            card,
            target: to,
            amount,
        });
        // The Counter Step re-offers on the next tick, so the defender may keep
        // countering (7-1-3-2).
        self.state.pending = None;
        Ok(())
    }

    /// `[Counter]` Event cards in hand the defender can afford (7-1-3-2-2).
    pub fn legal_counter_events(&self) -> Vec<CardInstanceId> {
        let Some(b) = &self.state.battle else {
            return Vec::new();
        };
        let defender = self.state.card(b.attacker).controller.opponent();
        let affordable = self.active_don(defender).len();
        self.state
            .player(defender)
            .hand
            .iter()
            .copied()
            .filter(|&c| {
                let def = self.db.get(self.state.card(c).def);
                let script = self.scripts.script(self.state.card(c).def);
                def.category == Category::Event
                    && !script.counter.is_empty()
                    && def.cost as usize <= affordable
                    // 8-3-1-3: an extra cost the [Counter] text names has to be
                    // payable in full or the Event does nothing at all.
                    && (script.counter_cost.is_free()
                        || self.can_pay(defender, c, &script.counter_cost))
            })
            .collect()
    }

    fn resolve_counter_event(
        &mut self,
        player: PlayerId,
        card: CardInstanceId,
        to: CardInstanceId,
        events: &mut Vec<GameEvent>,
    ) -> Result<(), IllegalAction> {
        if !self.legal_counter_events().contains(&card) {
            return Err(IllegalAction::Illegal(
                "that card is not a playable [Counter] Event".into(),
            ));
        }
        if self.state.card(to).controller != player
            || !matches!(self.state.card(to).zone, Zone::Leader | Zone::Character)
        {
            return Err(IllegalAction::Illegal(
                "[Counter] can only boost your own Leader or Character".into(),
            ));
        }

        let cost = self.db.get(self.state.card(card).def).cost;
        for don in self.active_don(player).into_iter().take(cost as usize) {
            self.state.card_mut(don).rested = true;
        }

        let script = self.scripts.script(self.state.card(card).def);
        let ops = script.counter.clone();
        let extra = script.counter_cost.clone();
        let mut owed = None;
        if !extra.is_free() {
            // `legal_counter_events` already refused anything unpayable.
            owed = self.pay(player, card, &extra, &[], events);
        }
        // 8-4-2: the Event is trashed, then its effect is carried out.
        self.move_card(card, player, Zone::Trash, Placement::Top, events);
        events.push(GameEvent::CardPlayed {
            player,
            card,
            cost_paid: cost,
        });

        let mut frame = EffectFrame::new(card, player, ops);
        frame.bind(crate::script::TARGET_BINDING, vec![to]);
        self.state.resolution.push(frame);

        // Clearing `pending` lets the frame resolve first; the Counter Step then
        // re-offers, so the defender may keep countering (7-1-3-2). A `DON!! −X`
        // selection stands in front of that — the Counter Step is still there
        // once it is answered.
        self.state.pending = owed;
        Ok(())
    }

    fn resolve_damage_step(&mut self, events: &mut Vec<GameEvent>) {
        let Some(b) = self.state.battle.clone() else {
            return;
        };
        let derived = self.derived();
        let attacker_power = derived.power(b.attacker);
        let target_power = derived.power(b.target);
        // 7-1-4-1: attacker wins ties.
        let won = attacker_power >= target_power;

        events.push(GameEvent::BattleResolved {
            attacker: b.attacker,
            target: b.target,
            attacker_power,
            target_power,
            attacker_won: won,
        });

        if !won {
            // 7-1-4-2: nothing happens.
            self.enter_battle_step(BattleStep::EndOfBattle, events);
            return;
        }

        match self.db.get(self.state.card(b.target).def).category {
            Category::Leader => {
                let victim = self.state.card(b.target).controller;
                let attacker_ch = derived.get(b.attacker);
                let amount = if attacker_ch.has_keyword(Keyword::DoubleAttack) {
                    2
                } else {
                    1
                };
                events.push(GameEvent::DamageDealt {
                    player: victim,
                    amount,
                });
                self.state.damage = Some(DamageState {
                    player: victim,
                    remaining: amount,
                    banish: attacker_ch.has_keyword(Keyword::Banish),
                });
            }
            _ => {
                // 7-1-4-1-2: the Character is K.O.'d — unless it is protected
                // from exactly this. ST08-002's protection is narrower than
                // `cannot_be_koed_by_effect`: it stops the *battle* K.O., and
                // only when a Leader won the battle.
                let attacker_is_leader =
                    self.db.get(self.state.card(b.attacker).def).category == Category::Leader;
                let protected =
                    attacker_is_leader && derived.get(b.target).cannot_be_koed_in_battle_by_leader;
                if !protected {
                    self.knock_out(b.target, events);
                }
                self.enter_battle_step(BattleStep::EndOfBattle, events);
            }
        }
    }

    /// Applies one point of damage, suspending for a `[Trigger]` decision if
    /// the revealed life card has one (8-6-2-1, 10-1-5).
    fn apply_one_damage(&mut self, events: &mut Vec<GameEvent>) {
        let Some(dmg) = self.state.damage else {
            return;
        };
        if dmg.remaining == 0 {
            self.state.damage = None;
            self.enter_battle_step(BattleStep::EndOfBattle, events);
            return;
        }

        // 7-1-4-1-1-1: taking damage at 0 Life loses the game. Checked before
        // the card is moved, per "at the point when it is determined that
        // damage will be dealt".
        if self.state.player(dmg.player).life.is_empty() {
            self.state.damage = None;
            self.end_game(GameOver::LifeDepleted { loser: dmg.player }, events);
            return;
        }

        let card = self.state.player(dmg.player).life[0];
        // Gated on the *script*, not the printed text: an unscripted Trigger
        // would offer a choice that does nothing, which is worse than treating
        // the card as having no Trigger. The coverage report tracks the gap.
        let has_trigger = !self
            .scripts
            .script(self.state.card(card).def)
            .trigger
            .is_empty();

        if has_trigger && !dmg.banish {
            // Suspend and ask. The card belongs to no area while its Trigger is
            // being considered (10-1-5-3).
            self.state.pending = Some(Pending::Trigger {
                player: dmg.player,
                card,
            });
            return;
        }

        self.take_life_card(dmg.player, card, dmg.banish, events);
        if let Some(d) = &mut self.state.damage {
            d.remaining -= 1;
        }
    }

    fn take_life_card(
        &mut self,
        player: PlayerId,
        card: CardInstanceId,
        banish: bool,
        events: &mut Vec<GameEvent>,
    ) {
        let to = if banish { Zone::Trash } else { Zone::Hand };
        self.move_card(card, player, to, Placement::Top, events);
        events.push(GameEvent::LifeTaken {
            player,
            card,
            banished: banish,
        });
    }

    fn resolve_trigger(
        &mut self,
        player: PlayerId,
        card: CardInstanceId,
        use_it: bool,
        events: &mut Vec<GameEvent>,
    ) {
        self.state.pending = None;
        if use_it {
            events.push(GameEvent::TriggerActivated { player, card });
            let mut ops = self
                .scripts
                .script(self.state.card(card).def)
                .trigger
                .clone();
            // 10-1-5-3: while its Trigger is being activated the card belongs
            // to no area, and afterwards it is trashed unless the Trigger says
            // otherwise. A Trigger that plays the card moves it out of Limbo
            // itself, so only cards still in Limbo get trashed.
            self.state.lift(card);
            ops.push(crate::effect::EffectOp::TrashIfInLimbo);
            let mut frame = EffectFrame::new(card, player, ops);
            frame.bind(crate::script::TARGET_BINDING, vec![card]);
            self.state.resolution.push(frame);
        } else {
            self.take_life_card(player, card, false, events);
        }
        if let Some(d) = &mut self.state.damage {
            d.remaining -= 1;
        }
    }

    /// [`GameState::move_card`], reporting the DON!! it sent back (6-5-5-4): a
    /// cost area that grows on its own reads like one that was never spent.
    ///
    /// Every move in this file goes through here, so none can detach DON!!
    /// silently. It emits no [`GameEvent::CardMoved`] — most callers already
    /// name the move they made, and only the rest push one.
    ///
    /// The detach therefore lands just *before* the K.O. or move that explains
    /// it. Reporting it after would read better and is not worth the price:
    /// the caller would have to remember to flush it, which is the forgetting
    /// this exists to prevent, and `from` names the card either way.
    fn move_card(
        &mut self,
        id: CardInstanceId,
        to_controller: PlayerId,
        to: Zone,
        placement: Placement,
        events: &mut Vec<GameEvent>,
    ) {
        for don in self.state.move_card(id, to_controller, to, placement) {
            let player = self.state.card(don).owner;
            events.push(GameEvent::DonDetached {
                player,
                don,
                from: id,
            });
        }
    }

    fn knock_out(&mut self, card: CardInstanceId, events: &mut Vec<GameEvent>) {
        let was_character = self.db.get(self.state.card(card).def).category == Category::Character;
        let owner = self.state.card(card).owner;
        self.move_card(card, owner, Zone::Trash, Placement::Top, events);
        events.push(GameEvent::KnockedOut { card });

        // "When a Character is K.O.'d" watches the whole board, not the card
        // that left it, so every card in play gets the timing (ST08-001).
        // Trashing a Character to make room for a sixth (3-7-6-1) is not a
        // K.O. and does not come through here.
        if was_character {
            for watcher in self.state.all_in_play() {
                self.queue_autos(Timing::OnCharacterKoed, watcher, events);
            }
        }
    }

    fn end_battle(&mut self, events: &mut Vec<GameEvent>) {
        // 7-1-5-2: "at the end of this battle" effects activate, before the
        // battle-scoped modifiers are cleared.
        if let Some(b) = self.state.battle.clone() {
            let target_is_character =
                self.db.get(self.state.card(b.target).def).category == Category::Character;
            if target_is_character {
                // Both participants, because "a battle in which this Character
                // battles your opponent's Character" (ST02-010, ST08-013) reads
                // the same whichever side declared the attack. A participant
                // the battle just K.O.'d has left its area, and 8-1-3-1-3 stops
                // its effect there.
                for (participant, other) in [(b.attacker, b.target), (b.target, b.attacker)] {
                    if self.state.card(participant).zone.is_field() {
                        self.queue_autos_with(
                            Timing::EndOfBattle,
                            participant,
                            &[(BATTLED_BINDING, vec![other])],
                            events,
                        );
                    }
                }
            }
        }
        // 7-1-5-3/4: effects lasting "during this battle" become invalid.
        self.state
            .modifiers
            .retain(|m| m.duration != Duration::ThisBattle);
        self.state.battle = None;
        self.state.damage = None;
        events.push(GameEvent::BattleEnded);
    }

    // ---- the forward loop --------------------------------------------------

    /// Runs the engine until it needs a decision or the game ends.
    fn advance(&mut self, events: &mut Vec<GameEvent>) {
        // Bounded so a rules bug surfaces as a panic in tests rather than a
        // hang in a training run.
        for _ in 0..10_000 {
            if self.state.game_over.is_some() || self.state.pending.is_some() {
                return;
            }
            if self.run_rule_processing(events) {
                return;
            }
            if !self.tick(events) {
                return;
            }
        }
        panic!(
            "engine failed to reach a decision point in 10000 ticks\n  phase={:?} turn={} tp={:?}\n  battle={:?}\n  damage={:?}\n  resolution={:?}",
            self.state.phase,
            self.state.turn,
            self.state.turn_player,
            self.state.battle,
            self.state.damage,
            self.state.resolution,
        );
    }

    /// One unit of forward progress. Returns false when the engine is parked.
    fn tick(&mut self, events: &mut Vec<GameEvent>) -> bool {
        // Suspended effect resolution takes priority over everything else.
        if !self.state.resolution.is_empty() {
            self.resolve_current_frame(events);
            return true;
        }
        if self.state.damage.is_some() {
            self.apply_one_damage(events);
            return true;
        }
        if self.state.battle.is_some() {
            self.tick_battle(events);
            return true;
        }
        self.tick_phase(events)
    }

    fn tick_battle(&mut self, events: &mut Vec<GameEvent>) {
        let Some(b) = self.state.battle.clone() else {
            return;
        };

        // Between every pair of steps: if a participant left its area, jump to
        // the end of the battle (7-1-1-4, 7-1-2-3, 7-1-3-3).
        if b.step != BattleStep::EndOfBattle && !self.battle_participants_intact() {
            self.enter_battle_step(BattleStep::EndOfBattle, events);
            return;
        }

        match b.step {
            BattleStep::Attack => {
                // 7-1-1-3: [When Attacking] and the defender's
                // [On Your Opponent's Attack] effects activate here.
                self.queue_autos(Timing::WhenAttacking, b.attacker, events);
                let defender = self.state.card(b.attacker).controller.opponent();
                for card in self.state.battlers(defender) {
                    self.queue_autos(Timing::OnYourOpponentsAttack, card, events);
                }
                self.enter_battle_step(BattleStep::Block, events);
            }
            BattleStep::Block => {
                let defender = self.state.card(b.attacker).controller.opponent();
                if self.legal_blockers().is_empty() {
                    self.enter_battle_step(BattleStep::Counter, events);
                } else {
                    self.state.pending = Some(Pending::Block { player: defender });
                }
            }
            BattleStep::Counter => {
                let defender = self.state.card(b.attacker).controller.opponent();
                if self.legal_counters().is_empty() && self.legal_counter_events().is_empty() {
                    self.enter_battle_step(BattleStep::Damage, events);
                } else {
                    self.state.pending = Some(Pending::Counter { player: defender });
                }
            }
            BattleStep::Damage => {
                self.resolve_damage_step(events);
            }
            BattleStep::EndOfBattle => {
                self.end_battle(events);
            }
        }
    }

    /// Advances the turn machine. Returns false only when parked on a decision.
    fn tick_phase(&mut self, events: &mut Vec<GameEvent>) -> bool {
        // Setup: both players mulligan, then Life is placed and turn 1 begins.
        if self.state.turn == 0 {
            let first = self.state.first_player;
            if !self.state.player(first).mulliganed {
                self.state.pending = Some(Pending::Mulligan { player: first });
                return true;
            }
            if !self.state.player(first.opponent()).mulliganed {
                self.state.pending = Some(Pending::Mulligan {
                    player: first.opponent(),
                });
                return true;
            }
            self.place_life(events);
            self.begin_turn(events);
            return true;
        }

        match self.state.phase {
            Phase::Refresh => {
                self.refresh_phase(events);
                self.state.phase = Phase::Draw;
                events.push(GameEvent::PhaseStarted {
                    phase: Phase::Draw,
                    player: self.state.turn_player,
                });
            }
            Phase::Draw => {
                self.draw_phase(events);
                if self.state.game_over.is_some() {
                    return true;
                }
                self.state.phase = Phase::Don;
                events.push(GameEvent::PhaseStarted {
                    phase: Phase::Don,
                    player: self.state.turn_player,
                });
            }
            Phase::Don => {
                self.don_phase(events);
                self.state.phase = Phase::Main;
                events.push(GameEvent::PhaseStarted {
                    phase: Phase::Main,
                    player: self.state.turn_player,
                });
            }
            Phase::Main => {
                self.state.pending = Some(Pending::MainAction {
                    player: self.state.turn_player,
                });
            }
            Phase::End => {
                // Two visits: the first queues end-of-turn effects, and the
                // queue is drained by `tick` before the second tears down
                // turn-scoped state. 6-6-1-1 must fully resolve before 6-6-1-3.
                if !self.state.end_autos_queued {
                    self.end_phase(events);
                    self.state.end_autos_queued = true;
                } else {
                    self.end_phase_cleanup();
                    self.state.end_autos_queued = false;
                    self.begin_turn(events);
                }
            }
        }
        true
    }

    fn begin_turn(&mut self, events: &mut Vec<GameEvent>) {
        self.state.turn += 1;
        // Turn 1 belongs to the first player; afterwards it alternates.
        if self.state.turn > 1 {
            self.state.turn_player = self.state.turn_player.opponent();
        } else {
            self.state.turn_player = self.state.first_player;
        }
        self.state.turns_taken[self.state.turn_player.index()] += 1;
        self.state.phase = Phase::Refresh;
        events.push(GameEvent::TurnStarted {
            turn: self.state.turn,
            player: self.state.turn_player,
        });
        events.push(GameEvent::PhaseStarted {
            phase: Phase::Refresh,
            player: self.state.turn_player,
        });
    }

    fn refresh_phase(&mut self, events: &mut Vec<GameEvent>) {
        let player = self.state.turn_player;

        // 6-2-1: effects lasting until the start of your turn end. The
        // "until your next turn" duration expires on its controller's turn.
        self.state
            .modifiers
            .retain(|m| !(m.duration == Duration::UntilYourNextTurn && m.controller == player));

        // 6-2-3: DON!! given to cards return to the cost area, rested.
        let mut returned = 0u8;
        for id in self.state.battlers(player) {
            let count = self.state.card(id).attached_don.len();
            if count > 0 {
                self.state.detach_don(id);
                returned += count as u8;
            }
        }
        if returned > 0 {
            events.push(GameEvent::DonReturned {
                player,
                count: returned,
            });
        }

        // 6-2-4: everything in your field is set active.
        for zone in [Zone::Leader, Zone::Character, Zone::Stage, Zone::Cost] {
            for id in self.state.player(player).zone(zone).to_vec() {
                if self.state.card(id).rested {
                    self.state.card_mut(id).rested = false;
                    events.push(GameEvent::SetActive { card: id });
                }
            }
        }
    }

    fn draw_phase(&mut self, events: &mut Vec<GameEvent>) {
        let player = self.state.turn_player;
        // 6-3-1: the player going first does not draw on their first turn.
        if self.state.turn == 1 && player == self.state.first_player {
            return;
        }
        // 9-2-1-2 is checked by rule processing; drawing from an empty deck
        // simply does nothing here.
        if let Some(card) = draw_one(&mut self.state, player) {
            events.push(GameEvent::Drew { player, card });
        }
    }

    fn don_phase(&mut self, events: &mut Vec<GameEvent>) {
        let player = self.state.turn_player;
        // 6-4-1: 2 DON!!, but only 1 on the first player's first turn.
        let want = if self.state.turn == 1 && player == self.state.first_player {
            1
        } else {
            2
        };
        let mut placed = 0u8;
        for _ in 0..want {
            // 6-4-2/6-4-3: place what is there, or nothing.
            let Some(don) = self.state.player(player).don_deck.first().copied() else {
                break;
            };
            self.state.lift(don);
            self.state.put(don, player, Zone::Cost, Placement::Bottom);
            self.state.card_mut(don).rested = false;
            placed += 1;
        }
        if placed > 0 {
            events.push(GameEvent::DonPlaced {
                player,
                count: placed,
            });
        }
    }

    fn end_phase(&mut self, events: &mut Vec<GameEvent>) {
        // 6-6-1-1: [End of Your Turn] resolves before [End of Your Opponent's
        // Turn] (6-6-1-1-2), turn player's first within each group.
        let turn_player = self.state.turn_player;
        for card in self.state.all_in_play() {
            if self.state.card(card).controller == turn_player {
                self.queue_autos(Timing::EndOfYourTurn, card, events);
            }
        }
        for card in self.state.all_in_play() {
            if self.state.card(card).controller != turn_player {
                self.queue_autos(Timing::EndOfYourOpponentsTurn, card, events);
            }
        }
    }

    /// The teardown half of the End Phase, run once end-of-turn effects have
    /// finished resolving.
    fn end_phase_cleanup(&mut self) {
        // 6-6-1-3: effects lasting "during this turn" become invalid.
        self.state
            .modifiers
            .retain(|m| m.duration != Duration::ThisTurn);
        // [Once Per Turn] bookkeeping resets for everyone.
        for id in self.state.all_in_play() {
            self.state.card_mut(id).used_once_per_turn.clear();
        }
    }

    fn place_life(&mut self, events: &mut Vec<GameEvent>) {
        for idx in 0..2 {
            let player = PlayerId(idx as u8);
            let Some(leader) = self.state.player(player).leader else {
                continue;
            };
            let life = self.db.get(self.state.card(leader).def).life.unwrap_or(0);
            // 5-2-1-7: cards go in such that the card at the top of the deck
            // ends up at the *bottom* of the Life area. Since index 0 of the
            // Life area is its top — the card taken on damage — each card drawn
            // is placed on top of the ones before it, leaving the first card
            // drawn at the bottom.
            for _ in 0..life {
                let Some(card) = self.state.player(player).deck.first().copied() else {
                    break;
                };
                self.state.lift(card);
                self.state.put(card, player, Zone::Life, Placement::Top);
            }
            events.push(GameEvent::LifeSet {
                player,
                count: life,
            });
        }
    }

    fn mulligan(&mut self, player: PlayerId, events: &mut Vec<GameEvent>) {
        // 5-2-1-6-1: return the hand, reshuffle, redraw 5.
        let hand = std::mem::take(&mut self.state.player_mut(player).hand);
        for card in hand {
            self.state.card_mut(card).zone = Zone::Limbo;
            self.state.put(card, player, Zone::Deck, Placement::Bottom);
        }
        let mut deck = std::mem::take(&mut self.state.players[player.index()].deck);
        deck.shuffle(&mut self.state.rng);
        self.state.players[player.index()].deck = deck;
        for _ in 0..5 {
            if let Some(card) = draw_one(&mut self.state, player) {
                events.push(GameEvent::Drew { player, card });
            }
        }
    }

    // ---- rule processing ---------------------------------------------------

    /// Rule processing (9). Runs before every tick and resolves immediately.
    /// Returns true if the game ended.
    fn run_rule_processing(&mut self, events: &mut Vec<GameEvent>) -> bool {
        // 9-2-1-2: a player with an empty deck loses. Checked for both players
        // so a simultaneous condition is a draw (9-2-1).
        let p0_out = self.state.player(PlayerId::P0).deck.is_empty();
        let p1_out = self.state.player(PlayerId::P1).deck.is_empty();
        let result = match (p0_out, p1_out) {
            (true, true) => Some(GameOver::Draw),
            (true, false) => Some(GameOver::DeckOut {
                loser: PlayerId::P0,
            }),
            (false, true) => Some(GameOver::DeckOut {
                loser: PlayerId::P1,
            }),
            (false, false) => None,
        };
        if let Some(result) = result {
            self.end_game(result, events);
            return true;
        }
        false
    }

    fn end_game(&mut self, result: GameOver, events: &mut Vec<GameEvent>) {
        self.state.game_over = Some(result);
        self.state.pending = None;
        events.push(GameEvent::GameEnded { result });
    }

    // ---- effect resolution -------------------------------------------------

    /// Executes the frame at the front of the queue until it suspends for a
    /// choice or completes.
    ///
    /// Suspension leaves `ip` pointing *at* the op that needs input, so that
    /// resuming re-runs it — by which time the answer is in `bindings` and it
    /// falls through.
    ///
    /// An op may queue further effects — an `[On Play]` triggered by playing a
    /// card, a `[When a Character is K.O.'d]` triggered by K.O.ing one. Those
    /// join the back of the queue and are not reached until this frame retires,
    /// which is 8-6-3. Nothing pops the front while this runs, so the frame
    /// being executed stays the current one throughout.
    fn resolve_current_frame(&mut self, events: &mut Vec<GameEvent>) {
        loop {
            let Some(frame) = self.state.resolution.current().cloned() else {
                return;
            };

            // An unpaid cost is a question, not a fee.
            if let Some(pc) = &frame.pending_cost {
                self.state.pending = Some(Pending::PayCost {
                    player: frame.controller,
                    source: frame.source,
                    cost: pc.cost.clone(),
                });
                return;
            }

            if frame.ip >= frame.ops.len() {
                self.state.resolution.retire();
                return;
            }

            let op = frame.ops[frame.ip].clone();
            match self.exec_op(&frame, op, events) {
                OpOutcome::Advance => {
                    if let Some(current) = self.state.resolution.current_mut() {
                        current.ip += 1;
                    }
                }
                OpOutcome::Suspend => return,
                OpOutcome::Retry => {}
                OpOutcome::Abort => {
                    self.state.resolution.retire();
                    return;
                }
            }
        }
    }

    /// Binds into the frame being executed.
    ///
    /// Ops enqueue new frames behind the current one, so "the frame being
    /// executed" is always the front of the queue — including from inside an op
    /// that has just triggered something else.
    fn bind_current(&mut self, key: &str, cards: Vec<CardInstanceId>) {
        if let Some(current) = self.state.resolution.current_mut() {
            current.bind(key, cards);
        }
    }

    fn exec_op(
        &mut self,
        frame: &crate::effect::EffectFrame,
        op: crate::effect::EffectOp,
        events: &mut Vec<GameEvent>,
    ) -> OpOutcome {
        use crate::effect::EffectOp as Op;

        match op {
            Op::Choose { key, select } => {
                if frame.has_binding(&key) {
                    return OpOutcome::Advance;
                }
                let options = self.selector_options(frame, &select);
                if options.is_empty() {
                    // Nothing to choose; bind empty and continue (8-4-4-1).
                    // Announced, because the cost has already been paid and
                    // silence looks like a bug.
                    events.push(GameEvent::NoLegalTargets {
                        source: frame.source,
                        controller: frame.controller,
                    });
                    self.bind_current(&key, Vec::new());
                    return OpOutcome::Advance;
                }
                // One legal answer is not a decision: a floor covering the
                // whole pool means every candidate, so do not stage a choice.
                let floor = (select.at_least as usize).min(options.len());
                if floor == options.len() {
                    self.bind_current(&key, options);
                    return OpOutcome::Advance;
                }
                self.state.pending = Some(Pending::Choose {
                    player: frame.controller,
                    key,
                    options,
                    up_to: select.up_to,
                    at_least: select.at_least,
                });
                OpOutcome::Suspend
            }

            Op::Modify {
                key,
                kind,
                duration,
            } => {
                for &target in frame.bound(&key) {
                    self.state.modifiers.push(Modifier {
                        target,
                        kind,
                        duration,
                        source: frame.source,
                        controller: frame.controller,
                    });
                }
                OpOutcome::Advance
            }

            Op::Ko { key } => {
                let derived = self.derived();
                for &target in frame.bound(&key) {
                    // 8-1-3-1-3: a card that already left its area is skipped.
                    if self.state.card(target).zone != Zone::Character {
                        continue;
                    }
                    // "Cannot be K.O.'d by effects" stops this but not a lost
                    // battle (10-2-1-1), which is why the check lives here and
                    // not in `knock_out`.
                    if derived.get(target).cannot_be_koed_by_effect {
                        continue;
                    }
                    self.knock_out(target, events);
                }
                OpOutcome::Advance
            }

            Op::Rest { key } => {
                for &target in frame.bound(&key) {
                    if !self.state.card(target).rested {
                        self.state.card_mut(target).rested = true;
                        events.push(GameEvent::Rested { card: target });
                    }
                }
                OpOutcome::Advance
            }

            Op::SetActive { key } => {
                for &target in frame.bound(&key) {
                    if self.state.card(target).rested {
                        self.state.card_mut(target).rested = false;
                        events.push(GameEvent::SetActive { card: target });
                    }
                }
                OpOutcome::Advance
            }

            Op::Draw { player, n } => {
                for who in self.players_of(frame.controller, player) {
                    for _ in 0..n {
                        match draw_one(&mut self.state, who) {
                            Some(card) => events.push(GameEvent::Drew { player: who, card }),
                            None => break,
                        }
                    }
                }
                OpOutcome::Advance
            }

            Op::GiveDon { key, n, source } => {
                let targets = frame.bound(&key).to_vec();
                for target in targets {
                    for _ in 0..n {
                        // `cost_area` holds only DON!! not already given —
                        // giving lifts them out of it — so a DON!! sitting
                        // under another Character is out of the pool already,
                        // which is the other half of the ST01-001 ruling.
                        let pool: Vec<CardInstanceId> = self
                            .state
                            .player(frame.controller)
                            .cost_area
                            .iter()
                            .copied()
                            .filter(|&d| source.admits(self.state.card(d).rested))
                            .collect();
                        let Some(don) = pool.first().copied() else {
                            break;
                        };
                        // Rest state is left alone: an effect selects a DON!!
                        // already in the state it names.
                        self.state.lift(don);
                        self.state.card_mut(don).zone = Zone::Cost;
                        self.state.card_mut(target).attached_don.push(don);
                        events.push(GameEvent::DonGiven {
                            player: frame.controller,
                            don,
                            to: target,
                        });
                    }
                }
                OpOutcome::Advance
            }

            Op::AddDon { n, rested } => {
                let mut added = 0;
                for _ in 0..n {
                    // 6-4-2/6-4-3: an empty DON!! deck simply supplies nothing.
                    let Some(don) = self
                        .state
                        .player(frame.controller)
                        .don_deck
                        .first()
                        .copied()
                    else {
                        break;
                    };
                    self.state.lift(don);
                    self.state
                        .put(don, frame.controller, Zone::Cost, Placement::Bottom);
                    self.state.card_mut(don).rested = rested;
                    added += 1;
                }
                if added > 0 {
                    events.push(GameEvent::DonPlaced {
                        player: frame.controller,
                        count: added,
                    });
                }
                OpOutcome::Advance
            }

            Op::TrashLife { player, n } => {
                for victim in self.players_of(frame.controller, player) {
                    for _ in 0..n {
                        let Some(&top) = self.state.player(victim).life.first() else {
                            break;
                        };
                        self.move_card(top, victim, Zone::Trash, Placement::Top, events);
                        // Not damage: no `[Trigger]` (10-1-5) and nothing to
                        // hand. Reported as a plain move rather than as
                        // `LifeTaken`, whose `banished` flag names the keyword
                        // (10-1-3) and is not what happened here.
                        events.push(GameEvent::CardMoved {
                            card: top,
                            from: Zone::Life,
                            to: Zone::Trash,
                        });
                    }
                }
                OpOutcome::Advance
            }

            Op::MoveTo { key, to } => {
                for &target in frame.bound(&key) {
                    let owner = self.state.card(target).owner;
                    let from = self.state.card(target).zone;
                    let placement = if to == Zone::Deck {
                        Placement::Bottom
                    } else {
                        Placement::Top
                    };
                    self.move_card(target, owner, to, placement, events);
                    // The one mover with nothing else to name it. A bounce read
                    // later without this looks like a card that never left, and
                    // trashing it from hand for its Counter reads as 7-1-3-2-1.
                    events.push(GameEvent::CardMoved {
                        card: target,
                        from,
                        to,
                    });
                }
                OpOutcome::Advance
            }

            Op::PlayBound { key } => {
                let targets = frame.bound(&key).to_vec();
                for card in targets {
                    let player = self.state.card(card).owner;
                    let category = self.db.get(self.state.card(card).def).category;
                    match category {
                        Category::Character => {
                            if self.state.player(player).characters.len() >= 5 {
                                continue;
                            }
                            self.move_card(
                                card,
                                player,
                                Zone::Character,
                                Placement::Bottom,
                                events,
                            );
                            self.state.card_mut(card).played_on_turn = Some(self.state.turn);
                            events.push(GameEvent::CardPlayed {
                                player,
                                card,
                                cost_paid: 0,
                            });
                            self.queue_autos(Timing::OnPlay, card, events);
                        }
                        Category::Stage => {
                            // 3-8-5-1 again, reached when an effect does the
                            // playing rather than the player.
                            if let Some(existing) = self.state.player(player).stage {
                                self.move_card(
                                    existing,
                                    player,
                                    Zone::Trash,
                                    Placement::Top,
                                    events,
                                );
                                events.push(GameEvent::CardMoved {
                                    card: existing,
                                    from: Zone::Stage,
                                    to: Zone::Trash,
                                });
                            }
                            self.move_card(card, player, Zone::Stage, Placement::Bottom, events);
                            events.push(GameEvent::CardPlayed {
                                player,
                                card,
                                cost_paid: 0,
                            });
                        }
                        _ => {}
                    }
                }
                OpOutcome::Advance
            }

            Op::DigTop {
                n,
                key,
                up_to,
                filters,
            } => {
                if frame.has_binding(&key) {
                    // The choice has been made: add it to hand, bottom the rest.
                    let chosen = frame.bound(&key).to_vec();
                    let looked: Vec<CardInstanceId> = frame.bound(&dig_pool_key(&key)).to_vec();
                    for card in looked {
                        let player = self.state.card(card).owner;
                        if chosen.contains(&card) {
                            self.move_card(card, player, Zone::Hand, Placement::Bottom, events);
                        } else {
                            self.move_card(card, player, Zone::Deck, Placement::Bottom, events);
                        }
                    }
                    return OpOutcome::Advance;
                }

                // First visit: take the top n off the deck into limbo so they
                // cannot be drawn while the choice is pending.
                let pool: Vec<CardInstanceId> = self
                    .state
                    .player(frame.controller)
                    .deck
                    .iter()
                    .take(n as usize)
                    .copied()
                    .collect();
                for &card in &pool {
                    self.state.lift(card);
                }
                let derived = self.derived();
                let options: Vec<CardInstanceId> = pool
                    .iter()
                    .copied()
                    .filter(|&c| {
                        derive::matches_filters(
                            &self.state,
                            &self.db,
                            &derived,
                            frame.source,
                            c,
                            &filters,
                        )
                    })
                    .collect();

                self.bind_current(&dig_pool_key(&key), pool);
                if options.is_empty() {
                    self.bind_current(&key, Vec::new());
                    // Re-run the op so the bottoming branch above executes.
                    return OpOutcome::Retry;
                }
                self.state.pending = Some(Pending::Choose {
                    player: frame.controller,
                    key,
                    options,
                    up_to,
                    // "reveal *up to* 1" — a dig may always be declined.
                    at_least: 0,
                });
                OpOutcome::Suspend
            }

            Op::TrashIfInLimbo => {
                if self.state.card(frame.source).zone == Zone::Limbo {
                    let owner = self.state.card(frame.source).owner;
                    self.state
                        .put(frame.source, owner, Zone::Trash, Placement::Top);
                }
                OpOutcome::Advance
            }

            Op::LookTop { n, key } => {
                if frame.has_binding(&key) {
                    // The placement was performed by the action handler; the
                    // binding exists only to say the question was answered.
                    return OpOutcome::Advance;
                }
                // Lifted into limbo for the same reason `DigTop` does it: the
                // cards must not be drawable while the decision is pending.
                let pool: Vec<CardInstanceId> = self
                    .state
                    .player(frame.controller)
                    .deck
                    .iter()
                    .take(n as usize)
                    .copied()
                    .collect();
                if pool.is_empty() {
                    self.bind_current(&key, Vec::new());
                    return OpOutcome::Advance;
                }
                for &card in &pool {
                    self.state.lift(card);
                }
                self.state.pending = Some(Pending::Arrange {
                    player: frame.controller,
                    cards: pool,
                    key,
                });
                OpOutcome::Suspend
            }

            Op::Shuffle { player } => {
                for p in self.players_of(frame.controller, player) {
                    // Through `state.rng` like every other shuffle: a game is a
                    // pure function of (config, seed, actions), and a second
                    // randomness source would break replay and the debug logs.
                    let mut deck = std::mem::take(&mut self.state.players[p.index()].deck);
                    deck.shuffle(&mut self.state.rng);
                    self.state.players[p.index()].deck = deck;
                }
                OpOutcome::Advance
            }

            Op::RequireIf { cond } => {
                let holds = derive::conditions_hold(
                    &self.state,
                    &self.db,
                    &self.derived(),
                    frame.source,
                    Some(frame),
                    std::slice::from_ref(&cond),
                );
                if holds {
                    OpOutcome::Advance
                } else {
                    // 8-3-3: nothing after the "if" clause resolves.
                    OpOutcome::Abort
                }
            }
        }
    }

    /// The players a [`Who`] names, relative to an effect's controller.
    ///
    /// A list rather than one player because card text that says plain
    /// "Characters" reaches both sides (ST08-005).
    ///
    /// [`Who`]: crate::effect::Who
    fn players_of(&self, controller: PlayerId, who: crate::effect::Who) -> Vec<PlayerId> {
        match who {
            crate::effect::Who::You => vec![controller],
            crate::effect::Who::Opponent => vec![controller.opponent()],
            // Turn player first, so the pool order does not depend on which
            // seat happens to control the effect.
            crate::effect::Who::Both => {
                let first = self.state.turn_player;
                vec![first, first.opponent()]
            }
        }
    }

    /// Whether an activated effect could currently affect anything.
    ///
    /// False when the effect asks for targets and none of its selectors match.
    /// The rules permit activating anyway — "up to 1" allows choosing zero
    /// (8-4-4-1) — and the cost is still paid (8-4-1-3), so this is advice for
    /// the UI and for agents, deliberately *not* a legality check.
    pub fn activation_finds_targets(&self, card: CardInstanceId, slot: u8) -> bool {
        let Some(effect) = self
            .scripts
            .script(self.state.card(card).def)
            .activated
            .iter()
            .find(|a| a.slot == slot)
        else {
            return true;
        };

        let controller = self.state.card(card).controller;
        let frame = EffectFrame::new(card, controller, Vec::new());
        let mut asked = false;
        for op in &effect.ops {
            if let crate::effect::EffectOp::Choose { select, .. } = op {
                asked = true;
                if !self.selector_options(&frame, select).is_empty() {
                    return true;
                }
            }
        }
        // An effect that never asks for a target always does something.
        !asked
    }

    /// The first choice an activated effect would ask, before it is activated.
    ///
    /// So a client can ask the question *before* sending the activation and
    /// send both together, the way an attack is offered as one action per
    /// (attacker, target) pair. `[Once Per Turn]` is spent by activating
    /// (8-4-1-3), so a target chosen afterwards is chosen too late to back out
    /// of; a target chosen first costs nothing to abandon.
    ///
    /// `None` when the effect asks nothing, or when a condition it would stop
    /// at does not hold. True only of the moment it is asked: the pool is read
    /// before the cost is paid, so a client that pre-selects from it must be
    /// ready for the engine to offer something narrower once the effect runs.
    pub fn activation_choice(
        &self,
        card: CardInstanceId,
        slot: u8,
    ) -> Option<ActivationChoice<'_>> {
        let effect = self
            .scripts
            .script(self.state.card(card).def)
            .activated
            .iter()
            .find(|a| a.slot == slot)?;

        let controller = self.state.card(card).controller;
        let frame = EffectFrame::new(card, controller, Vec::new());

        for op in &effect.ops {
            match op {
                // A condition the effect will stop at describes no choice: the
                // `choose` after it is never reached. Offering its pool anyway
                // invites a player to pick a target the effect will not use.
                crate::effect::EffectOp::RequireIf { cond }
                    if decidable(cond) && !self.holds(&frame, cond) =>
                {
                    return None
                }
                // Its pool is a binding the effect has not made yet, so there is
                // nothing to describe and nothing to read into its emptiness.
                // `activation_shortfall` passes over the same case.
                crate::effect::EffectOp::Choose { select, .. } if select.from.is_some() => {
                    return None
                }
                crate::effect::EffectOp::Choose { select, .. } => {
                    // A pool the controller cannot see is not describable
                    // without handing them its contents. Ids are assigned in
                    // decklist order, so shipping one for a deck card names it
                    // to anyone holding the decklist — and the count alone
                    // still answers "would searching be worth it" before the
                    // cost that buys the answer.
                    let secret = !select.zone.is_open();
                    return Some(ActivationChoice {
                        select,
                        options: if secret {
                            Vec::new()
                        } else {
                            self.selector_options(&frame, select)
                        },
                        secret,
                    });
                }
                _ => {}
            }
        }
        None
    }

    /// Whether a condition holds for an effect that has not started resolving.
    ///
    /// The same call `Op::RequireIf` makes, so the two agree about what would
    /// stop the effect.
    fn holds(&self, frame: &EffectFrame, cond: &crate::effect::Condition) -> bool {
        derive::conditions_hold(
            &self.state,
            &self.db,
            &self.derived(),
            frame.source,
            Some(frame),
            std::slice::from_ref(cond),
        )
    }

    /// The first thing an activated effect needs and does not currently have.
    ///
    /// For warning a player *before* the action is sent. `[Once Per Turn]` is
    /// spent by activating (8-4-1-3) — `activate_effect` marks the slot before
    /// the effect resolves — so a warning that arrives with the choice has
    /// arrived one action too late.
    ///
    /// Every requirement in the effect is checked, not only the first choice.
    /// ST01-001 is why: its `choose` picks the recipient, a pool that always
    /// holds at least the Leader, while the thing it can be short of is the
    /// rested DON!! its `give_don` draws afterwards.
    ///
    /// Same standing as [`Game::activation_finds_targets`]: advice, never a
    /// legality check — activating with nothing to affect is legal and still
    /// costs (8-4-1-3) — and true only of the moment it is asked. Pools are
    /// read before the cost is paid, and an op the effect has not reached yet
    /// is judged against a board it may itself change on the way there.
    pub fn activation_shortfall(&self, card: CardInstanceId, slot: u8) -> Option<Shortfall<'_>> {
        let effect = self
            .scripts
            .script(self.state.card(card).def)
            .activated
            .iter()
            .find(|a| a.slot == slot)?;

        let controller = self.state.card(card).controller;
        let frame = EffectFrame::new(card, controller, Vec::new());

        // Whether anything has happened by the time the effect reaches the op
        // being judged. `Choose` and `RequireIf` are plumbing — they decide what
        // the effect works on, and change nothing themselves.
        let mut done_something = false;

        for op in &effect.ops {
            let req = match op {
                // The effect stops here, so nothing after it happens.
                crate::effect::EffectOp::RequireIf { cond } => {
                    // A condition reading a binding cannot be judged from a
                    // frame that has none. Answering it "false" would put a
                    // permanent "condition not met" on a working ability.
                    if !decidable(cond) || self.holds(&frame, cond) {
                        continue;
                    }
                    Requirement::Condition
                }
                crate::effect::EffectOp::Choose { select, .. } => {
                    // Two pools that cannot be judged from here, and must not be
                    // guessed about. A secret area would have to be read to be
                    // reported on, which is the leak. A `from` pool is a binding
                    // the effect has not made yet, so it is empty now and says
                    // nothing about what it will hold.
                    if !select.zone.is_open() || select.from.is_some() {
                        continue;
                    }
                    if !self.selector_options(&frame, select).is_empty() {
                        continue;
                    }
                    Requirement::Cards(select)
                }
                // The pool `Op::GiveDon` will draw from, computed the same way
                // it computes it: `cost_area` holds only DON!! not already
                // given.
                crate::effect::EffectOp::GiveDon { source, .. } => {
                    let none = self
                        .state
                        .player(controller)
                        .cost_area
                        .iter()
                        .all(|&d| !source.admits(self.state.card(d).rested));
                    if !none {
                        done_something = true;
                        continue;
                    }
                    Requirement::Don(*source)
                }
                _ => {
                    done_something = true;
                    continue;
                }
            };
            return Some(Shortfall {
                req,
                sole: !done_something,
            });
        }
        None
    }

    /// Whether playing `card` from hand would find a target for its text.
    ///
    /// Covers Events, whose `[Main]` effect resolves on play, and the `[On Play]`
    /// effects of Characters and Stages. Same caveat as
    /// [`Game::activation_finds_targets`]: advice for the UI and for agents,
    /// never a legality check.
    pub fn play_finds_targets(&self, card: CardInstanceId) -> bool {
        let def = self.state.card(card).def;
        let script = self.scripts.script(def);
        let controller = self.state.card(card).controller;
        let frame = EffectFrame::new(card, controller, Vec::new());

        // An Event's [Main] effect is its first activated entry (10-2-3).
        let event_ops = if self.db.get(def).category == Category::Event {
            script.activated.first().map(|a| a.ops.as_slice())
        } else {
            None
        };
        let on_play_ops = script
            .auto
            .iter()
            .filter(|a| a.timing == Timing::OnPlay)
            .map(|a| a.ops.as_slice());

        let mut asked = false;
        for ops in event_ops.into_iter().chain(on_play_ops) {
            for op in ops {
                if let crate::effect::EffectOp::Choose { select, .. } = op {
                    asked = true;
                    if !self.selector_options(&frame, select).is_empty() {
                        return true;
                    }
                }
            }
        }
        !asked
    }

    /// Cards currently satisfying a selector.
    fn selector_options(
        &self,
        frame: &crate::effect::EffectFrame,
        select: &crate::effect::Selector,
    ) -> Vec<CardInstanceId> {
        let derived = self.derived();
        let keep = |me: &Self, c: CardInstanceId| {
            derive::matches_filters(
                &me.state,
                &me.db,
                &derived,
                frame.source,
                c,
                &select.filters,
            )
        };

        // A pool carried by the frame, for candidates the state can no longer
        // re-derive. Only cards still where the effect can act on them:
        // 8-1-3-1-3 drops the rest.
        if let Some(from) = &select.from {
            return frame
                .bound(from)
                .iter()
                .copied()
                .filter(|&c| self.state.card(c).zone.is_field() && keep(self, c))
                .collect();
        }

        let mut pool: Vec<CardInstanceId> = Vec::new();
        for owner in self.players_of(frame.controller, select.owner) {
            match select.zone {
                // "your Leader or 1 of your Characters" is by far the most
                // common target set, and spans two areas.
                Zone::Leader => pool.extend(self.state.battlers(owner)),
                other => pool.extend(self.state.player(owner).zone(other).iter().copied()),
            }
        }
        pool.into_iter().filter(|&c| keep(self, c)).collect()
    }

    /// Pushes any auto effects on `card` whose timing has just been met
    /// (8-1-3-1). Conditions are checked at activation time (8-3-2).
    fn queue_autos(&mut self, timing: Timing, card: CardInstanceId, events: &mut Vec<GameEvent>) {
        self.queue_autos_with(timing, card, &[], events);
    }

    /// As [`Game::queue_autos`], with extra bindings pre-supplied to the frame.
    ///
    /// For context the effect cannot recover from the state once it resolves —
    /// see [`BATTLED_BINDING`].
    fn queue_autos_with(
        &mut self,
        timing: Timing,
        card: CardInstanceId,
        supplied: &[(&str, Vec<CardInstanceId>)],
        events: &mut Vec<GameEvent>,
    ) {
        let def = self.state.card(card).def;
        let controller = self.state.card(card).controller;
        let effects: Vec<_> = self
            .scripts
            .script(def)
            .auto
            .iter()
            .filter(|a| a.timing == timing)
            .cloned()
            .collect();

        // Built on first need and then reused: queuing a frame changes nothing
        // derivation reads, and most autos have no condition to read it — this
        // runs once per card in play on every K.O. and every end of turn.
        let mut derived: Option<Derived> = None;
        for effect in effects {
            if effect.once_per_turn
                && self
                    .state
                    .card(card)
                    .used_once_per_turn
                    .contains(&effect.slot)
            {
                continue;
            }
            if !effect.conditions.is_empty() {
                let table = derived.get_or_insert_with(|| self.derived());
                if !derive::conditions_hold(
                    &self.state,
                    &self.db,
                    table,
                    card,
                    None,
                    &effect.conditions,
                ) {
                    continue;
                }
            }
            // 8-3-1-3: a cost that cannot be paid in full cannot be paid at all,
            // so an unaffordable one stops the effect here rather than asking.
            let mut pending_cost = None;
            if !effect.cost.is_free() {
                if !self.can_pay(controller, card, &effect.cost) {
                    continue;
                }
                // 8-3-1-4: the cost is the controller's to decline. Pushed
                // unpaid; it announces itself only once paid.
                pending_cost = Some(crate::effect::PendingCost {
                    cost: effect.cost.clone(),
                    slot: effect.slot,
                    once_per_turn: effect.once_per_turn,
                });
            } else {
                if effect.once_per_turn {
                    self.state
                        .card_mut(card)
                        .used_once_per_turn
                        .push(effect.slot);
                }
                events.push(GameEvent::EffectActivated {
                    source: card,
                    controller,
                });
            }
            let mut frame = EffectFrame::new(card, controller, effect.ops.clone());
            frame.pending_cost = pending_cost;
            for (key, cards) in supplied {
                frame.bind(key, cards.clone());
            }
            self.state.resolution.push(frame);
        }
    }

    // ---- helpers -----------------------------------------------------------

    /// Active DON!! in a player's cost area, in a stable order.
    pub fn active_don(&self, player: PlayerId) -> Vec<CardInstanceId> {
        self.state
            .player(player)
            .cost_area
            .iter()
            .copied()
            .filter(|&d| self.state.card(d).is_active())
            .collect()
    }
}

fn draw_one(state: &mut GameState, player: PlayerId) -> Option<CardInstanceId> {
    let card = state.player(player).deck.first().copied()?;
    state.lift(card);
    state.put(card, player, Zone::Hand, Placement::Bottom);
    Some(card)
}

fn validate_deck(deck: &DeckList) -> Result<(), SetupError> {
    // 5-1-2: exactly 50 cards.
    if deck.cards.len() != 50 {
        return Err(SetupError::DeckSize(deck.cards.len()));
    }
    // 5-1-2-3: no more than 4 with the same card number.
    let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
    for number in &deck.cards {
        let entry = counts.entry(number.as_str()).or_default();
        *entry += 1;
        if *entry > 4 {
            return Err(SetupError::TooManyCopies(number.clone()));
        }
    }
    Ok(())
}
