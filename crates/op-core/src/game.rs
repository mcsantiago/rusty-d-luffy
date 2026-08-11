//! The engine: setup, the turn machine, the battle machine, and the step loop.

use std::sync::Arc;

use rand::seq::SliceRandom;

use crate::action::{Action, IllegalAction, Pending};
use crate::card::{CardDb, Category, Keyword};
use crate::derive::{self, Derived};
use crate::effect::{Duration, EffectFrame, ModKind, Modifier, Timing};
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

            (Pending::PayCost { player, .. }, Action::PayCost(pay)) => {
                let player = *player;
                let idx =
                    self.state
                        .stack
                        .frames
                        .len()
                        .checked_sub(1)
                        .ok_or(IllegalAction::Illegal(
                            "no effect is waiting on a cost".into(),
                        ))?;
                let Some(pc) = self.state.stack.frames[idx].pending_cost.take() else {
                    return Err(IllegalAction::Illegal(
                        "that effect is not waiting on a cost".into(),
                    ));
                };
                let source = self.state.stack.frames[idx].source;
                if !pay {
                    // 8-3-1-4: refused, so the effect never activates.
                    self.state.stack.frames.remove(idx);
                    self.state.pending = None;
                    return Ok(());
                }
                // An earlier effect in the same window may have spent it.
                if !self.can_pay(player, source, &pc.cost) {
                    self.state.stack.frames.remove(idx);
                    self.state.pending = None;
                    return Ok(());
                }
                // Simplification: a hand cost takes the leftmost cards.
                let discard: Vec<CardInstanceId> = self
                    .state
                    .player(player)
                    .hand
                    .iter()
                    .take(pc.cost.trash_from_hand as usize)
                    .copied()
                    .collect();
                self.pay(player, source, &pc.cost, &discard, events);
                if pc.once_per_turn {
                    self.state.card_mut(source).used_once_per_turn.push(pc.slot);
                }
                events.push(GameEvent::EffectActivated {
                    source,
                    controller: player,
                });
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
                let _ = player;
                if let Some(frame) = self.state.stack.top_mut() {
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
            self.state
                .move_card(victim, player, Zone::Trash, Placement::Top);
            events.push(GameEvent::KnockedOut { card: victim });
        }

        match category {
            Category::Character => {
                self.state
                    .move_card(card, player, Zone::Character, Placement::Bottom);
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
                if let Some(existing) = self.state.player(player).stage {
                    self.state
                        .move_card(existing, player, Zone::Trash, Placement::Top);
                }
                self.state
                    .move_card(card, player, Zone::Stage, Placement::Bottom);
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
                // 8-3-1-3: a cost that cannot be paid in full cannot be paid at
                // all, and then the effect does not resolve. The card is still
                // played and trashed.
                //
                // Diverges from "You may ...:" in one direction only: with the
                // cost payable, the engine pays. Declining would resolve
                // nothing at all for DON!! already spent, so it is never a
                // choice a player would take — and playing the Event is itself
                // the decision. Same simplification as an auto effect with a
                // cost; see `queue_autos`.
                let paying = extra.is_free() || self.can_pay(player, card, &extra);
                let ops = match &main {
                    Some(a) if paying => a.ops.clone(),
                    _ => Vec::new(),
                };
                if paying && !extra.is_free() {
                    let discard: Vec<CardInstanceId> = self
                        .state
                        .player(player)
                        .hand
                        .iter()
                        .filter(|&&c| c != card)
                        .take(extra.trash_from_hand as usize)
                        .copied()
                        .collect();
                    self.pay(player, card, &extra, &discard, events);
                }
                self.state
                    .move_card(card, player, Zone::Trash, Placement::Top);
                events.push(GameEvent::CardPlayed {
                    player,
                    card,
                    cost_paid: cost,
                });
                if !ops.is_empty() {
                    events.push(GameEvent::EffectActivated {
                        source: card,
                        controller: player,
                    });
                    self.state.stack.push(EffectFrame::new(card, player, ops));
                }
            }
            Category::Leader | Category::Don => {
                return Err(IllegalAction::Illegal("that card cannot be played".into()))
            }
        }

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
        if !derive::conditions_hold(&self.state, &self.db, &[], card, None, &effect.conditions) {
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
        if discard.len() != effect.cost.trash_from_hand as usize {
            return Err(IllegalAction::Illegal(format!(
                "this costs {} card(s) from hand, {} named",
                effect.cost.trash_from_hand,
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
        self.pay(player, card, &effect.cost, discard, events);

        if effect.once_per_turn {
            self.state.card_mut(card).used_once_per_turn.push(slot);
        }
        events.push(GameEvent::EffectActivated {
            source: card,
            controller: player,
        });
        self.state
            .stack
            .push(EffectFrame::new(card, player, effect.ops));
        self.state.pending = None;
        Ok(())
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
        if self.state.player(player).cost_area.len() < cost.don_minus as usize {
            return false;
        }
        true
    }

    fn pay(
        &mut self,
        player: PlayerId,
        card: CardInstanceId,
        cost: &crate::script::ActivationCost,
        discard: &[CardInstanceId],
        events: &mut Vec<GameEvent>,
    ) {
        for don in self
            .active_don(player)
            .into_iter()
            .take(cost.rest_don as usize)
        {
            self.state.card_mut(don).rested = true;
        }
        if cost.rest_self {
            self.state.card_mut(card).rested = true;
            events.push(GameEvent::Rested { card });
        }
        for &card in discard {
            self.state
                .move_card(card, player, Zone::Trash, Placement::Top);
        }
        // Life cards come off the top and go to hand face-up. This is not
        // damage, so no `[Trigger]` activates (10-1-5 ties Triggers to damage).
        for _ in 0..cost.life_to_hand {
            let Some(&top) = self.state.player(player).life.first() else {
                break;
            };
            self.state
                .move_card(top, player, Zone::Hand, Placement::Top);
            events.push(GameEvent::LifeTaken {
                player,
                card: top,
                banished: false,
            });
        }
        if cost.don_minus > 0 {
            let spent = self.return_don(player, cost.don_minus);
            events.push(GameEvent::DonSpentToDonDeck {
                player,
                count: spent,
            });
        }
    }

    /// Sends `n` DON!! from `player`'s cost area back to the bottom of their
    /// DON!! deck, and reports how many actually went.
    ///
    /// Rested DON!! go first. The player is never asked which: an active DON!!
    /// can still be spent this turn and a rested one cannot, so keeping the
    /// active ones is the choice they would make every time.
    fn return_don(&mut self, player: PlayerId, n: u8) -> u8 {
        let mut pool: Vec<CardInstanceId> = self.state.player(player).cost_area.to_vec();
        pool.sort_by_key(|&d| self.state.card(d).is_active());
        let mut returned = 0;
        for don in pool.into_iter().take(n as usize) {
            self.state.lift(don);
            self.state.card_mut(don).rested = false;
            self.state
                .put(don, player, Zone::DonDeck, Placement::Bottom);
            returned += 1;
        }
        returned
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
        if let Some(blocker) = blocker {
            let legal = self.legal_blockers();
            if !legal.contains(&blocker) {
                return Err(IllegalAction::Illegal("that card cannot block".into()));
            }
            let old_target = self.state.battle.as_ref().unwrap().target;
            // 10-1-4-1: resting the blocker makes it the new target.
            self.state.card_mut(blocker).rested = true;
            events.push(GameEvent::Rested { card: blocker });
            if let Some(b) = &mut self.state.battle {
                b.target = blocker;
                b.blocker_used = true;
            }
            events.push(GameEvent::Blocked {
                blocker,
                replacing: old_target,
            });
            // 7-1-2-2 and 10-2-15-1: activating a [Blocker] is what fulfils
            // [On Block], so it fires here rather than from a battle step —
            // the Block Step is also entered when nobody blocks.
            //
            // Queued before the step advances, so the frames resolve while the
            // battle still reads as the Block Step. If one of them moves the
            // attacker or the new target, `tick_battle`'s standing check sees
            // it against the zones snapshotted on entering the Counter Step and
            // jumps to the end of the battle, which is 7-1-2-3.
            self.queue_autos(Timing::OnBlock, blocker, events);
        }
        let _ = player;
        self.state.pending = None;
        self.enter_battle_step(BattleStep::Counter, events);
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

        self.state
            .move_card(card, player, Zone::Trash, Placement::Top);
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
        if !extra.is_free() {
            // `legal_counter_events` already refused anything unpayable.
            self.pay(player, card, &extra, &[], events);
        }
        // 8-4-2: the Event is trashed, then its effect is carried out.
        self.state
            .move_card(card, player, Zone::Trash, Placement::Top);
        events.push(GameEvent::CardPlayed {
            player,
            card,
            cost_paid: cost,
        });

        let mut frame = EffectFrame::new(card, player, ops);
        frame.bind(crate::script::TARGET_BINDING, vec![to]);
        self.state.stack.push(frame);

        // Clearing `pending` lets the frame resolve first; the Counter Step then
        // re-offers, so the defender may keep countering (7-1-3-2).
        self.state.pending = None;
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
        self.state.move_card(card, player, to, Placement::Top);
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
            let ops = self
                .scripts
                .script(self.state.card(card).def)
                .trigger
                .clone();
            // 10-1-5-3: while its Trigger is being activated the card belongs
            // to no area, and afterwards it is trashed unless the Trigger says
            // otherwise. A Trigger that plays the card moves it out of Limbo
            // itself, so only cards still in Limbo get trashed.
            self.state.lift(card);
            let mut frame = EffectFrame::new(card, player, ops);
            frame.bind(crate::script::TARGET_BINDING, vec![card]);
            self.state.stack.push(frame);
            self.state
                .stack
                .frames
                .last_mut()
                .unwrap()
                .ops
                .push(crate::effect::EffectOp::TrashIfInLimbo);
        } else {
            self.take_life_card(player, card, false, events);
        }
        if let Some(d) = &mut self.state.damage {
            d.remaining -= 1;
        }
    }

    fn knock_out(&mut self, card: CardInstanceId, events: &mut Vec<GameEvent>) {
        let was_character = self.db.get(self.state.card(card).def).category == Category::Character;
        let owner = self.state.card(card).owner;
        self.state
            .move_card(card, owner, Zone::Trash, Placement::Top);
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
            "engine failed to reach a decision point in 10000 ticks\n  phase={:?} turn={} tp={:?}\n  battle={:?}\n  damage={:?}\n  stack={:?}",
            self.state.phase,
            self.state.turn,
            self.state.turn_player,
            self.state.battle,
            self.state.damage,
            self.state.stack,
        );
    }

    /// One unit of forward progress. Returns false when the engine is parked.
    fn tick(&mut self, events: &mut Vec<GameEvent>) -> bool {
        // Suspended effect resolution takes priority over everything else.
        if !self.state.stack.is_empty() {
            self.resolve_top_frame(events);
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
                // stack is drained by `tick` before the second tears down
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

    /// Executes the top frame's ops until it suspends for a choice or completes.
    ///
    /// Suspension leaves `ip` pointing *at* the op that needs input, so that
    /// resuming re-runs it — by which time the answer is in `bindings` and it
    /// falls through.
    fn resolve_top_frame(&mut self, events: &mut Vec<GameEvent>) {
        loop {
            let Some(frame) = self.state.stack.frames.last().cloned() else {
                return;
            };
            // An op may push further frames (an [On Play] triggered by playing a
            // card, say), so the frame being executed is addressed by index
            // rather than as "the top" — otherwise its instruction pointer would
            // never advance and the effect would replay forever.
            let idx = self.state.stack.frames.len() - 1;

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
                self.state.stack.frames.remove(idx);
                return;
            }

            let op = frame.ops[frame.ip].clone();
            match self.exec_op(idx, &frame, op, events) {
                OpOutcome::Advance => self.state.stack.frames[idx].ip += 1,
                OpOutcome::Suspend => return,
                OpOutcome::Retry => {}
                OpOutcome::Abort => {
                    self.state.stack.frames.remove(idx);
                    return;
                }
            }
        }
    }

    fn exec_op(
        &mut self,
        idx: usize,
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
                    self.state.stack.frames[idx].bind(&key, Vec::new());
                    return OpOutcome::Advance;
                }
                // One legal answer is not a decision: a floor covering the
                // whole pool means every candidate, so do not stage a choice.
                let floor = (select.at_least as usize).min(options.len());
                if floor == options.len() {
                    self.state.stack.frames[idx].bind(&key, options);
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
                        self.state
                            .move_card(top, victim, Zone::Trash, Placement::Top);
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
                    let placement = if to == Zone::Deck {
                        Placement::Bottom
                    } else {
                        Placement::Top
                    };
                    self.state.move_card(target, owner, to, placement);
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
                            self.state
                                .move_card(card, player, Zone::Character, Placement::Bottom);
                            self.state.card_mut(card).played_on_turn = Some(self.state.turn);
                            events.push(GameEvent::CardPlayed {
                                player,
                                card,
                                cost_paid: 0,
                            });
                            self.queue_autos(Timing::OnPlay, card, events);
                        }
                        Category::Stage => {
                            if let Some(existing) = self.state.player(player).stage {
                                self.state
                                    .move_card(existing, player, Zone::Trash, Placement::Top);
                            }
                            self.state
                                .move_card(card, player, Zone::Stage, Placement::Bottom);
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
                            self.state
                                .move_card(card, player, Zone::Hand, Placement::Bottom);
                        } else {
                            self.state
                                .move_card(card, player, Zone::Deck, Placement::Bottom);
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

                self.state.stack.frames[idx].bind(&dig_pool_key(&key), pool);
                if options.is_empty() {
                    self.state.stack.frames[idx].bind(&key, Vec::new());
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

            Op::RequireIf { cond } => {
                let derived = self.derived();
                let holds = derive::conditions_hold(
                    &self.state,
                    &self.db,
                    &[],
                    frame.source,
                    Some(frame),
                    std::slice::from_ref(&cond),
                );
                let _ = derived;
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
            if !derive::conditions_hold(&self.state, &self.db, &[], card, None, &effect.conditions)
            {
                continue;
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
            self.state.stack.push(frame);
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
