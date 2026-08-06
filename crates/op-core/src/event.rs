//! The append-only event log.
//!
//! Events are the only thing sent to clients (after per-player filtering) and
//! the only thing a UI needs to render a game. They describe what happened, not
//! what the state now is.

use serde::{Deserialize, Serialize};

use crate::ids::{CardInstanceId, PlayerId};
use crate::state::{BattleStep, GameOver, Phase};
use crate::zone::Zone;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
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
    GameEnded {
        result: GameOver,
    },
}
