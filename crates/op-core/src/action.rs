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
    /// Pay an auto effect's activation cost, or decline it (8-3-1-4).
    PayCost(bool),
    /// Name the DON!! cards a `DON!! −X` returns to the DON!! deck (8-3-1-6).
    /// Which ones is the player's choice (3-9-2), so it is its own decision
    /// rather than travelling with the action that incurred the cost — the
    /// alternative is a cross product with the hand cost beside it.
    ReturnDon { dons: Vec<CardInstanceId> },
    /// Answer a "return them to the top or bottom in any order" request
    /// (ST03-010). Both lists read top-to-bottom within their own group, and
    /// together they must name every card that was looked at, exactly once.
    Arrange {
        top: Vec<CardInstanceId>,
        bottom: Vec<CardInstanceId>,
    },
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
    /// 8-3-1-4: an auto effect's cost, which the controller may decline.
    /// Activated effects never land here; choosing to activate is the agreement.
    PayCost {
        player: PlayerId,
        /// The card whose effect is asking.
        source: CardInstanceId,
        /// What it wants, so a client can name the price.
        cost: crate::script::ActivationCost,
    },
    /// 8-3-1-6: which DON!! a `DON!! −X` takes back.
    ///
    /// Asked only when there is something to decide — a pool the same size as
    /// the cost has exactly one answer and is paid without asking, the same
    /// principle as [`Pending::Choose`]'s floor.
    ReturnDon {
        player: PlayerId,
        /// The card whose cost this is, so a client can name what is asking.
        source: CardInstanceId,
        /// How many must go. Never more than `options.len()`.
        n: u8,
        /// Every DON!! that may be taken: the cost area plus DON!! already
        /// given to this player's Leader and Characters.
        options: Vec<CardInstanceId>,
    },
    /// Cards taken off the top of the deck and awaiting placement, top-first as
    /// they were drawn. "Look at N and return them to the top or bottom of the
    /// deck in any order" (ST03-010).
    ///
    /// The cards are in `Zone::Limbo` while this is pending, so nothing can
    /// draw them mid-decision. Only the controller ever sees this — `project`
    /// drops a `Pending` belonging to the other player — which is what keeps
    /// "look at" from telling the opponent the top of your deck.
    Arrange {
        player: PlayerId,
        cards: Vec<CardInstanceId>,
        /// Binding key the resuming op checks to know the answer has arrived.
        key: String,
    },
    Choose {
        player: PlayerId,
        /// Binding key the answer is stored under in the suspended frame.
        key: String,
        /// Cards that satisfy the selector right now.
        options: Vec<CardInstanceId>,
        up_to: u8,
        /// Fewest cards a legal answer may name. 0 for the usual "up to N"
        /// (8-4-4-1); non-zero where the text is an instruction rather than an
        /// offer, e.g. "trash 1 card from your hand" (ST04-005).
        ///
        /// Capped at the number of options: a mandatory choice with too few
        /// legal cards takes as many as there are rather than deadlocking.
        at_least: u8,
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
            | Pending::PayCost { player, .. }
            | Pending::ReturnDon { player, .. }
            | Pending::Arrange { player, .. }
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
