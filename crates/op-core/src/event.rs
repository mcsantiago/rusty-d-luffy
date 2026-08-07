//! The event log, in two forms.
//!
//! [`GameEvent`] is what the engine emits: omniscient, full fidelity, and
//! **server-only**. [`PlayerEvent`] is what a client receives, produced by
//! [`GameEvent::project`], with the identity of any card the viewer is not
//! entitled to know replaced by [`CardRef::Hidden`].
//!
//! The split exists because a `CardInstanceId` is not an opaque token. Ids are
//! assigned in decklist order at setup, and decklists are public in
//! competitive play, so an id is close to a direct function of the card number.
//! Leaking one for a card in hand or deck leaks the card.
//!
//! Projection fails closed: a variant whose visibility is not obviously
//! justified redacts. Each rule below cites why the viewer is or is not
//! entitled to the identity.

use serde::{Deserialize, Serialize};

use crate::ids::{CardInstanceId, PlayerId};
use crate::state::{BattleStep, GameOver, GameState, Phase};
use crate::zone::Zone;

/// A card as referred to in a [`PlayerEvent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CardRef {
    /// The viewer may know which card this is, and can resolve the id against
    /// their [`crate::view::PlayerView`].
    Visible(CardInstanceId),
    /// Something happened to a card the viewer cannot identify — an opponent
    /// drawing, a card going to a deck. Deliberately carries no id.
    Hidden,
}

impl CardRef {
    pub fn id(self) -> Option<CardInstanceId> {
        match self {
            CardRef::Visible(id) => Some(id),
            CardRef::Hidden => None,
        }
    }

    pub fn is_hidden(self) -> bool {
        self == CardRef::Hidden
    }
}

/// Whether `viewer` may know the identity of `card` right now.
///
/// True in exactly two cases: the card sits in an open area (3-1-5), or it is
/// in the viewer's own hand, which they may look at freely (3-4-2). Notably a
/// player may *not* identify cards in their own deck (3-2-2) or their own Life
/// area, so ownership alone is not enough.
fn identifiable(state: &GameState, viewer: PlayerId, card: CardInstanceId) -> bool {
    let card = state.card(card);
    card.zone.is_open() || (card.zone == Zone::Hand && card.controller == viewer)
}

fn card_ref(state: &GameState, viewer: PlayerId, card: CardInstanceId) -> CardRef {
    if identifiable(state, viewer, card) {
        CardRef::Visible(card)
    } else {
        CardRef::Hidden
    }
}

/// What the engine emits. Omniscient — never send this to a client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameEvent {
    GameStarted {
        first_player: PlayerId,
    },
    Mulliganed {
        player: PlayerId,
        took: bool,
    },
    LifeSet {
        player: PlayerId,
        count: u8,
    },

    TurnStarted {
        turn: u32,
        player: PlayerId,
    },
    PhaseStarted {
        phase: Phase,
        player: PlayerId,
    },

    Drew {
        player: PlayerId,
        card: CardInstanceId,
    },
    DonPlaced {
        player: PlayerId,
        count: u8,
    },
    DonGiven {
        player: PlayerId,
        don: CardInstanceId,
        to: CardInstanceId,
    },
    DonReturned {
        player: PlayerId,
        count: u8,
    },

    CardPlayed {
        player: PlayerId,
        card: CardInstanceId,
        cost_paid: u8,
    },
    CardMoved {
        card: CardInstanceId,
        from: Zone,
        to: Zone,
    },
    Rested {
        card: CardInstanceId,
    },
    SetActive {
        card: CardInstanceId,
    },
    KnockedOut {
        card: CardInstanceId,
    },

    AttackDeclared {
        attacker: CardInstanceId,
        target: CardInstanceId,
    },
    BattleStepStarted {
        step: BattleStep,
    },
    Blocked {
        blocker: CardInstanceId,
        replacing: CardInstanceId,
    },
    Countered {
        player: PlayerId,
        card: CardInstanceId,
        target: CardInstanceId,
        amount: i32,
    },
    /// The power comparison at the Damage Step (7-1-4-1).
    BattleResolved {
        attacker: CardInstanceId,
        target: CardInstanceId,
        attacker_power: i32,
        target_power: i32,
        attacker_won: bool,
    },
    DamageDealt {
        player: PlayerId,
        amount: u8,
    },
    LifeTaken {
        player: PlayerId,
        card: CardInstanceId,
        /// True when [Banish] sent it to the trash instead of hand (10-1-3).
        banished: bool,
    },
    TriggerActivated {
        player: PlayerId,
        card: CardInstanceId,
    },
    BattleEnded,

    EffectActivated {
        source: CardInstanceId,
        controller: PlayerId,
    },
    /// An effect asked for a target and there were none. The cost has already
    /// been paid at this point (8-4-1-3), so the player is owed an explanation
    /// for why nothing happened.
    NoLegalTargets {
        source: CardInstanceId,
        controller: PlayerId,
    },
    GameEnded {
        result: GameOver,
    },
}

/// What a client receives. Safe to send to `viewer` by construction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayerEvent {
    GameStarted {
        first_player: PlayerId,
    },
    Mulliganed {
        player: PlayerId,
        took: bool,
    },
    LifeSet {
        player: PlayerId,
        count: u8,
    },

    TurnStarted {
        turn: u32,
        player: PlayerId,
    },
    PhaseStarted {
        phase: Phase,
        player: PlayerId,
    },

    Drew {
        player: PlayerId,
        card: CardRef,
    },
    DonPlaced {
        player: PlayerId,
        count: u8,
    },
    DonGiven {
        player: PlayerId,
        don: CardRef,
        to: CardRef,
    },
    DonReturned {
        player: PlayerId,
        count: u8,
    },

    CardPlayed {
        player: PlayerId,
        card: CardRef,
        cost_paid: u8,
    },
    CardMoved {
        card: CardRef,
        from: Zone,
        to: Zone,
    },
    Rested {
        card: CardRef,
    },
    SetActive {
        card: CardRef,
    },
    KnockedOut {
        card: CardRef,
    },

    AttackDeclared {
        attacker: CardRef,
        target: CardRef,
    },
    BattleStepStarted {
        step: BattleStep,
    },
    Blocked {
        blocker: CardRef,
        replacing: CardRef,
    },
    Countered {
        player: PlayerId,
        card: CardRef,
        target: CardRef,
        amount: i32,
    },
    BattleResolved {
        attacker: CardRef,
        target: CardRef,
        attacker_power: i32,
        target_power: i32,
        attacker_won: bool,
    },
    DamageDealt {
        player: PlayerId,
        amount: u8,
    },
    LifeTaken {
        player: PlayerId,
        card: CardRef,
        banished: bool,
    },
    TriggerActivated {
        player: PlayerId,
        card: CardRef,
    },
    BattleEnded,

    EffectActivated {
        source: CardRef,
        controller: PlayerId,
    },
    NoLegalTargets {
        source: CardRef,
        controller: PlayerId,
    },
    GameEnded {
        result: GameOver,
    },
}

impl GameEvent {
    /// Redacts this event for `viewer`.
    ///
    /// `state` must be the state *after* the event, which is what makes the
    /// zone-based rules correct: a card played from hand is in the Character
    /// area by the time anyone is told about it, and is therefore public.
    pub fn project(&self, state: &GameState, viewer: PlayerId) -> PlayerEvent {
        let vis = |card: CardInstanceId| card_ref(state, viewer, card);

        match *self {
            GameEvent::GameStarted { first_player } => PlayerEvent::GameStarted { first_player },
            GameEvent::Mulliganed { player, took } => {
                // 5-2-1-6 is performed openly; only the cards are secret.
                PlayerEvent::Mulliganed { player, took }
            }
            GameEvent::LifeSet { player, count } => PlayerEvent::LifeSet { player, count },
            GameEvent::TurnStarted { turn, player } => PlayerEvent::TurnStarted { turn, player },
            GameEvent::PhaseStarted { phase, player } => {
                PlayerEvent::PhaseStarted { phase, player }
            }

            // The drawing player may look at their own hand (3-4-2); nobody
            // else learns what left the deck.
            GameEvent::Drew { player, card } => PlayerEvent::Drew {
                player,
                card: if player == viewer {
                    CardRef::Visible(card)
                } else {
                    CardRef::Hidden
                },
            },

            GameEvent::DonPlaced { player, count } => PlayerEvent::DonPlaced { player, count },
            // The cost area and the cards DON!! is given to are open (3-1-5).
            GameEvent::DonGiven { player, don, to } => PlayerEvent::DonGiven {
                player,
                don: vis(don),
                to: vis(to),
            },
            GameEvent::DonReturned { player, count } => PlayerEvent::DonReturned { player, count },

            // A played card lands in an open area, or is an Event card trashed
            // on activation (8-4-2) — public either way.
            GameEvent::CardPlayed {
                player,
                card,
                cost_paid,
            } => PlayerEvent::CardPlayed {
                player,
                card: vis(card),
                cost_paid,
            },
            // Visible only if it ended somewhere the viewer may look. A card
            // moved into a deck becomes unidentifiable even to its owner.
            GameEvent::CardMoved { card, from, to } => PlayerEvent::CardMoved {
                card: vis(card),
                from,
                to,
            },
            GameEvent::Rested { card } => PlayerEvent::Rested { card: vis(card) },
            GameEvent::SetActive { card } => PlayerEvent::SetActive { card: vis(card) },
            GameEvent::KnockedOut { card } => PlayerEvent::KnockedOut { card: vis(card) },

            GameEvent::AttackDeclared { attacker, target } => PlayerEvent::AttackDeclared {
                attacker: vis(attacker),
                target: vis(target),
            },
            GameEvent::BattleStepStarted { step } => PlayerEvent::BattleStepStarted { step },
            GameEvent::Blocked { blocker, replacing } => PlayerEvent::Blocked {
                blocker: vis(blocker),
                replacing: vis(replacing),
            },
            // The Counter card is trashed to pay for the effect (7-1-3-2-1), so
            // it is face up in an open area by now.
            GameEvent::Countered {
                player,
                card,
                target,
                amount,
            } => PlayerEvent::Countered {
                player,
                card: vis(card),
                target: vis(target),
                amount,
            },
            GameEvent::BattleResolved {
                attacker,
                target,
                attacker_power,
                target_power,
                attacker_won,
            } => PlayerEvent::BattleResolved {
                attacker: vis(attacker),
                target: vis(target),
                attacker_power,
                target_power,
                attacker_won,
            },
            GameEvent::DamageDealt { player, amount } => {
                PlayerEvent::DamageDealt { player, amount }
            }
            // Banished life cards go to the open trash (10-1-3). Otherwise the
            // card enters its owner's hand and only they may see it.
            GameEvent::LifeTaken {
                player,
                card,
                banished,
            } => PlayerEvent::LifeTaken {
                player,
                card: vis(card),
                banished,
            },
            // 10-1-5-1: activating a [Trigger] reveals the card to both players.
            GameEvent::TriggerActivated { player, card } => PlayerEvent::TriggerActivated {
                player,
                card: CardRef::Visible(card),
            },
            GameEvent::BattleEnded => PlayerEvent::BattleEnded,

            // 8-4-1-2: activating an effect from hand reveals that card.
            GameEvent::EffectActivated { source, controller } => PlayerEvent::EffectActivated {
                source: CardRef::Visible(source),
                controller,
            },
            // The source is revealed by activating it, so naming it is safe.
            GameEvent::NoLegalTargets { source, controller } => PlayerEvent::NoLegalTargets {
                source: CardRef::Visible(source),
                controller,
            },
            GameEvent::GameEnded { result } => PlayerEvent::GameEnded { result },
        }
    }
}

impl PlayerEvent {
    /// Every card id this event exposes. Used by the leak tests.
    pub fn exposed_ids(&self) -> Vec<CardInstanceId> {
        let refs: Vec<CardRef> = match *self {
            PlayerEvent::Drew { card, .. }
            | PlayerEvent::CardPlayed { card, .. }
            | PlayerEvent::CardMoved { card, .. }
            | PlayerEvent::Rested { card }
            | PlayerEvent::SetActive { card }
            | PlayerEvent::KnockedOut { card }
            | PlayerEvent::LifeTaken { card, .. }
            | PlayerEvent::TriggerActivated { card, .. } => vec![card],
            PlayerEvent::DonGiven { don, to, .. } => vec![don, to],
            PlayerEvent::AttackDeclared { attacker, target } => vec![attacker, target],
            PlayerEvent::Blocked { blocker, replacing } => vec![blocker, replacing],
            PlayerEvent::Countered { card, target, .. } => vec![card, target],
            PlayerEvent::BattleResolved {
                attacker, target, ..
            } => vec![attacker, target],
            PlayerEvent::EffectActivated { source, .. }
            | PlayerEvent::NoLegalTargets { source, .. } => vec![source],
            _ => vec![],
        };
        refs.into_iter().filter_map(CardRef::id).collect()
    }
}
