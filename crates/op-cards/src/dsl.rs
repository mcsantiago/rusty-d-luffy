//! The card-scripting vocabulary.
//!
//! Effects are declared as data — the constructors below are a thin, readable
//! surface over `op_core::effect`. Expressing it in Rust rather than an external
//! file format keeps card scripts type-checked and lets an outlier card drop
//! into arbitrary code without a second mechanism; the trade-off is that adding
//! a card requires a rebuild.
//!
//! A card's script should read close to its printed text. Where it cannot, the
//! divergence is worth a comment naming the rule involved.

use op_core::card::{Category, Keyword};
use op_core::effect::{Condition, Duration, EffectOp, Filter, Selector, Who, SELF_BINDING};

// Re-exported so a card script only ever needs `use crate::dsl::*`.
pub use op_core::effect::Duration::{ThisBattle, ThisTurn};
pub use op_core::effect::{ModKind, Timing};
use op_core::script::{
    ActivatedEffect, ActivationCost, AutoEffect, CardScript, PermanentEffect, Scope,
};
use op_core::zone::Zone;

// ---- selectors -------------------------------------------------------------

/// "your Leader or N of your Characters" — the most common target set, spanning
/// the Leader and Character areas.
pub fn your_battlers(up_to: u8) -> Selector {
    Selector {
        zone: Zone::Leader,
        owner: Who::You,
        up_to,
        filters: Vec::new(),
    }
}

pub fn your_characters(up_to: u8) -> Selector {
    Selector {
        zone: Zone::Character,
        owner: Who::You,
        up_to,
        filters: Vec::new(),
    }
}

pub fn opponent_characters(up_to: u8) -> Selector {
    Selector {
        zone: Zone::Character,
        owner: Who::Opponent,
        up_to,
        filters: Vec::new(),
    }
}

pub fn your_don(up_to: u8) -> Selector {
    Selector {
        zone: Zone::Cost,
        owner: Who::You,
        up_to,
        filters: Vec::new(),
    }
}

pub fn opponent_don(up_to: u8) -> Selector {
    Selector {
        zone: Zone::Cost,
        owner: Who::Opponent,
        up_to,
        filters: Vec::new(),
    }
}

pub fn your_hand(up_to: u8) -> Selector {
    Selector {
        zone: Zone::Hand,
        owner: Who::You,
        up_to,
        filters: Vec::new(),
    }
}

/// Adds filters to a selector.
pub fn filtered(mut sel: Selector, filters: Vec<Filter>) -> Selector {
    sel.filters.extend(filters);
    sel
}

// ---- filters ---------------------------------------------------------------

pub fn cost_at_most(n: u8) -> Filter {
    Filter::CostAtMost(n)
}

pub fn power_at_most(n: i32) -> Filter {
    Filter::PowerAtMost(n)
}

pub fn of_type(types: &[&str]) -> Filter {
    Filter::HasAnyType(types.iter().map(|t| t.to_string()).collect())
}

pub fn rested(yes: bool) -> Filter {
    Filter::IsRested(yes)
}

pub fn with_keyword(kw: Keyword) -> Filter {
    Filter::HasKeyword(kw)
}

pub fn other_than_self() -> Filter {
    Filter::NotSelf
}

pub fn is_character() -> Filter {
    Filter::IsCategory(Category::Character)
}

// ---- ops -------------------------------------------------------------------

pub fn choose(key: &str, select: Selector) -> EffectOp {
    EffectOp::Choose {
        key: key.to_string(),
        select,
    }
}

pub fn power_up(key: &str, amount: i32, duration: Duration) -> EffectOp {
    EffectOp::Modify {
        key: key.to_string(),
        kind: ModKind::Power(amount),
        duration,
    }
}

pub fn cannot_be_blocked(key: &str, duration: Duration) -> EffectOp {
    EffectOp::Modify {
        key: key.to_string(),
        kind: ModKind::CannotBeBlocked,
        duration,
    }
}

pub fn blocker_ceiling(key: &str, power: i32, duration: Duration) -> EffectOp {
    EffectOp::Modify {
        key: key.to_string(),
        kind: ModKind::BlockerPowerCeiling(power),
        duration,
    }
}

pub fn ko(key: &str) -> EffectOp {
    EffectOp::Ko {
        key: key.to_string(),
    }
}

pub fn rest(key: &str) -> EffectOp {
    EffectOp::Rest {
        key: key.to_string(),
    }
}

pub fn set_active(key: &str) -> EffectOp {
    EffectOp::SetActive {
        key: key.to_string(),
    }
}

pub fn give_don(key: &str, n: u8, rested: bool) -> EffectOp {
    EffectOp::GiveDon {
        key: key.to_string(),
        n,
        rested,
    }
}

pub fn play_bound(key: &str) -> EffectOp {
    EffectOp::PlayBound {
        key: key.to_string(),
    }
}

pub fn dig_top(n: u8, key: &str, up_to: u8, filters: Vec<Filter>) -> EffectOp {
    EffectOp::DigTop {
        n,
        key: key.to_string(),
        up_to,
        filters,
    }
}

pub fn draw(n: u8) -> EffectOp {
    EffectOp::Draw {
        player: Who::You,
        n,
    }
}

/// The card the effect came from.
pub const THIS: &str = SELF_BINDING;

/// The card a `[Counter]` Event was pointed at.
pub const TARGET: &str = op_core::script::TARGET_BINDING;

// ---- conditions ------------------------------------------------------------

pub fn don(n: u8) -> Condition {
    Condition::DonAttached(n)
}

pub fn your_turn() -> Condition {
    Condition::YourTurn
}

pub fn characters_at_least(n: u8) -> Condition {
    Condition::CharacterCountAtLeast(n)
}

pub fn self_rested() -> Condition {
    Condition::SelfRested
}

// ---- effect builders -------------------------------------------------------

/// A permanent effect on the card itself.
pub fn permanent_self(conditions: Vec<Condition>, kind: ModKind) -> PermanentEffect {
    PermanentEffect {
        conditions,
        scope: Scope::ThisCard,
        kind,
    }
}

pub fn permanent_typed(
    conditions: Vec<Condition>,
    types: &[&str],
    kind: ModKind,
) -> PermanentEffect {
    PermanentEffect {
        conditions,
        scope: Scope::YourBattlersWithAnyType(types.iter().map(|t| t.to_string()).collect()),
        kind,
    }
}

pub fn auto(timing: Timing, conditions: Vec<Condition>, ops: Vec<EffectOp>) -> AutoEffect {
    AutoEffect {
        timing,
        conditions,
        ops,
        slot: 0,
        once_per_turn: false,
    }
}

pub fn auto_once(timing: Timing, conditions: Vec<Condition>, ops: Vec<EffectOp>) -> AutoEffect {
    AutoEffect {
        timing,
        conditions,
        ops,
        slot: 0,
        once_per_turn: true,
    }
}

pub fn activated(cost: ActivationCost, ops: Vec<EffectOp>) -> ActivatedEffect {
    ActivatedEffect {
        conditions: Vec::new(),
        cost,
        ops,
        slot: 0,
        once_per_turn: false,
    }
}

pub fn activated_once(cost: ActivationCost, ops: Vec<EffectOp>) -> ActivatedEffect {
    ActivatedEffect {
        conditions: Vec::new(),
        cost,
        ops,
        slot: 0,
        once_per_turn: true,
    }
}

pub fn free() -> ActivationCost {
    ActivationCost::default()
}

pub fn cost(rest_don: u8, rest_self: bool, trash_from_hand: u8) -> ActivationCost {
    ActivationCost {
        rest_don,
        rest_self,
        trash_from_hand,
    }
}

/// Builds a script from parts, filling in the empty defaults.
#[derive(Default)]
pub struct Script(CardScript);

impl Script {
    pub fn new() -> Script {
        Script(CardScript::default())
    }

    pub fn permanent(mut self, effect: PermanentEffect) -> Script {
        self.0.permanent.push(effect);
        self
    }

    pub fn auto(mut self, effect: AutoEffect) -> Script {
        self.0.auto.push(effect);
        self
    }

    pub fn activated(mut self, mut effect: ActivatedEffect) -> Script {
        effect.slot = self.0.activated.len() as u8;
        self.0.activated.push(effect);
        self
    }

    /// The `[Counter]` effect of an Event card (10-2-4).
    pub fn counter(mut self, ops: Vec<EffectOp>) -> Script {
        self.0.counter = ops;
        self
    }

    /// The `[Trigger]` effect (2-11).
    pub fn trigger(mut self, ops: Vec<EffectOp>) -> Script {
        self.0.trigger = ops;
        self
    }

    pub fn build(self) -> CardScript {
        self.0
    }
}
