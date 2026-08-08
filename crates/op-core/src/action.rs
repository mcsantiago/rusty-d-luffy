//! Player input and the questions the engine asks.

use serde::{Deserialize, Serialize};

use crate::ids::{CardInstanceId, PlayerId};

/// Something a player does. Every state transition that involves a decision
/// comes through here, which is what makes a game replayable as
/// `(config, seed, Vec<Action>)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    /// Take the one permitted mulligan, or keep (5-2-1-6).
    Mulligan(bool),

    /// Play a Character or Stage card, or activate a `[Main]` Event, from hand
    /// (6-5-3-1).
    PlayCard {
        card: CardInstanceId,
        /// A Character to trash first, when the Character area is already full
        /// (3-7-6-1). `None` when there is room.
        replacing: Option<CardInstanceId>,
    },
    /// Activate an `[Activate: Main]` effect on a card in play (6-5-4-1).
    ActivateEffect {
        card: CardInstanceId,
        slot: u8,
        /// Cards from hand to trash as part of the activation cost (8-3-1).
        /// Which ones is the player's choice, so it travels with the action.
        discard: Vec<CardInstanceId>,
    },
    /// Give one active DON!! from the cost area to a Leader or Character
    /// (6-5-5-1).
    GiveDon { to: CardInstanceId },
    /// Declare an attack (7-1-1).
    Attack {
        attacker: CardInstanceId,
        target: CardInstanceId,
    },
    /// Finish the Main Phase (6-5-2-1).
    EndMainPhase,

    /// Activate a `[Blocker]`, or decline (7-1-2).
    Block { blocker: Option<CardInstanceId> },

    /// Trash a Character from hand to add its Counter value to the defending
    /// card's power (7-1-3-2-1).
    Counter {
        card: CardInstanceId,
        to: CardInstanceId,
    },
    /// Activate a `[Counter]` Event from hand (7-1-3-2-2).
    CounterEvent {
        card: CardInstanceId,
        to: CardInstanceId,
    },
    /// Stop countering and proceed to the Damage Step.
    DoneCountering,

    /// Activate the revealed life card's `[Trigger]`, or take it to hand
    /// (10-1-5-2).
    UseTrigger(bool),

    /// Answer an effect's request to choose cards. May be empty when the text
    /// says "up to" (8-4-4-1).
    Choose { cards: Vec<CardInstanceId> },
}

/// What the engine is currently waiting for. Stored in `GameState` because a
/// suspended decision point *is* part of the game's position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pending {
    Mulligan {
        player: PlayerId,
    },
    MainAction {
        player: PlayerId,
    },
    Block {
        player: PlayerId,
    },
    Counter {
        player: PlayerId,
    },
    Trigger {
        player: PlayerId,
        card: CardInstanceId,
    },
    Choose {
        player: PlayerId,
        /// Binding key the answer is stored under in the suspended frame.
        key: String,
        /// Cards that satisfy the selector right now.
        options: Vec<CardInstanceId>,
        up_to: u8,
    },
}

impl Pending {
    pub fn player(&self) -> PlayerId {
        match self {
            Pending::Mulligan { player }
            | Pending::MainAction { player }
            | Pending::Block { player }
            | Pending::Counter { player }
            | Pending::Trigger { player, .. }
            | Pending::Choose { player, .. } => *player,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IllegalAction {
    #[error("the game is over")]
    GameOver,
    #[error("no decision is pending")]
    NothingPending,
    #[error("it is not player {0:?}'s decision")]
    WrongPlayer(PlayerId),
    #[error("action {action:?} does not answer the pending {pending:?}")]
    WrongKind { action: String, pending: String },
    #[error("{0}")]
    Illegal(String),
}
