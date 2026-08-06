//! The interface between the kernel and card scripts.
//!
//! `op-core` knows how to *execute* effects but not what any particular card
//! does. Scripts live in `op-cards` and are supplied through [`ScriptSource`],
//! keeping the kernel free of card data and the card crate free of rules
//! machinery.

use serde::{Deserialize, Serialize};

use crate::effect::{Condition, EffectOp, ModKind, Timing};
use crate::ids::CardDefId;

/// Which cards a permanent effect applies to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scope {
    /// "This Character gains …"
    ThisCard,
    /// "your {A} or {B} type Leaders and Characters gain …"
    YourBattlersWithAnyType(Vec<String>),
    /// "your Characters gain …"
    YourCharacters,
}

/// A permanent (continuous) effect — 8-1-3-3. Always in force while its
/// conditions hold; never stored as a modifier, always re-derived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermanentEffect {
    /// All conditions must hold (8-3-2-1).
    pub conditions: Vec<Condition>,
    pub scope: Scope,
    pub kind: ModKind,
}

/// An auto effect — 8-1-3-1. Activates on its timing, every time it occurs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoEffect {
    pub timing: Timing,
    pub conditions: Vec<Condition>,
    pub ops: Vec<EffectOp>,
    /// Distinguishes this effect from others on the same card for the purpose
    /// of `[Once Per Turn]` bookkeeping.
    pub slot: u8,
    pub once_per_turn: bool,
}

/// An activate effect — 8-1-3-2. `[Activate: Main]` / `[Main]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivatedEffect {
    pub conditions: Vec<Condition>,
    /// Activation cost paid before the effect resolves (8-3-1).
    pub cost: ActivationCost,
    pub ops: Vec<EffectOp>,
    pub slot: u8,
    pub once_per_turn: bool,
}

/// What must be paid to activate an effect (8-3-1).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivationCost {
    /// The ③ symbol: rest this many active DON!! in your cost area (8-3-1-5).
    pub rest_don: u8,
    /// Rest the source card itself ("You may rest this Character:").
    pub rest_self: bool,
    /// Trash this many cards from hand.
    pub trash_from_hand: u8,
}

impl ActivationCost {
    pub fn is_free(&self) -> bool {
        *self == ActivationCost::default()
    }
}

/// Everything the kernel needs to know about one card's behaviour.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardScript {
    pub permanent: Vec<PermanentEffect>,
    pub auto: Vec<AutoEffect>,
    /// Activate effects. For Event cards, the first entry is the `[Main]`
    /// effect, resolved when the card is played (10-2-3).
    pub activated: Vec<ActivatedEffect>,
    /// `[Counter]` effect ops for an Event card (10-2-4). The activation cost is
    /// the card's printed cost. The card the defender chose to boost is
    /// pre-bound under [`TARGET_BINDING`].
    pub counter: Vec<EffectOp>,
    /// `[Trigger]` ops (2-11). Resolved from the Life area on damage.
    pub trigger: Vec<EffectOp>,
}

/// Binding key under which the engine pre-supplies a target the player already
/// picked outside the effect, e.g. the card a `[Counter]` Event boosts.
pub const TARGET_BINDING: &str = "target";

impl CardScript {
    pub fn is_vanilla(&self) -> bool {
        self.permanent.is_empty()
            && self.auto.is_empty()
            && self.activated.is_empty()
            && self.counter.is_empty()
            && self.trigger.is_empty()
    }
}

/// Supplies scripts for printed cards.
pub trait ScriptSource {
    fn script(&self, def: CardDefId) -> &CardScript;
}

/// A [`ScriptSource`] under which every card is vanilla. The kernel is
/// exercised against this in M1 and in rules tests that only care about the
/// turn/battle machinery.
#[derive(Debug, Default, Clone)]
pub struct NoScripts {
    empty: CardScript,
}

impl ScriptSource for NoScripts {
    fn script(&self, _def: CardDefId) -> &CardScript {
        &self.empty
    }
}
