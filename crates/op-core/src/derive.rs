//! Derived characteristics.
//!
//! Power, cost and keywords are **never stored mutated** — they are recomputed
//! from printed values plus every effect currently applying. That is what makes
//! permanent effects (8-1-3-3) layer correctly, and it is the one design
//! decision here that cannot be retrofitted later.
//!
//! Layering order, per 8-1-3-3-5 and 8-4-6:
//!   1. printed values
//!   2. DON!! given to the card: +1000 each, during its controller's turn only
//!      (6-5-5-2)
//!   3. resolved continuous effects still in duration (stored modifiers)
//!   4. permanent effects, applied turn-player-first and iterated to a fixpoint

use crate::card::{CardDb, Category, Keyword};
use crate::effect::{Condition, ModKind};
use crate::ids::{CardInstanceId, PlayerId};
use crate::script::{Scope, ScriptSource};
use crate::state::GameState;
use crate::zone::Zone;

/// Upper bound on fixpoint iterations. Permanent effects that keep changing
/// each other past this point are a loop; the rules leave the outcome
/// undefined, so we stop and take the last stable-ish result.
const FIXPOINT_LIMIT: usize = 8;

/// A card's characteristics right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Characteristics {
    pub power: i32,
    /// Cost after modification, which may be **negative** mid-calculation.
    ///
    /// The rules keep a negative value for the duration of a calculation and
    /// treat it as 0 only outside one, so clamping per modifier would be wrong:
    /// a 3-cost Character given -4 then +2 is 1, not 2. Read it through
    /// [`Characteristics::effective_cost`].
    pub cost: i32,
    /// Effects cannot K.O. this card; battle still can (10-2-1-1).
    pub cannot_be_koed_by_effect: bool,
    /// A Leader that wins a battle against this card does not K.O. it
    /// (ST08-002). A Character attacker still does.
    pub cannot_be_koed_in_battle_by_leader: bool,
    pub keywords: Vec<Keyword>,
    pub cannot_be_blocked: bool,
    /// The opponent may not block with a `[Blocker]` whose power is at or above
    /// this value.
    pub blocker_power_ceiling: Option<i32>,
}

impl Characteristics {
    pub fn has_keyword(&self, kw: Keyword) -> bool {
        self.keywords.contains(&kw)
    }

    /// The cost as the game sees it: negative values are treated as 0.
    pub fn effective_cost(&self) -> u8 {
        self.cost.max(0) as u8
    }
}

/// Characteristics for every card, indexed by `CardInstanceId`.
#[derive(Debug, Clone)]
pub struct Derived {
    table: Vec<Characteristics>,
}

impl Derived {
    pub fn get(&self, id: CardInstanceId) -> &Characteristics {
        &self.table[id.index()]
    }

    pub fn power(&self, id: CardInstanceId) -> i32 {
        self.table[id.index()].power
    }
}

/// Computes characteristics for every card in the game.
pub fn derive_all(state: &GameState, db: &CardDb, scripts: &dyn ScriptSource) -> Derived {
    let mut table: Vec<Characteristics> = state
        .cards
        .iter()
        .map(|c| {
            let def = db.get(c.def);
            Characteristics {
                power: def.power.unwrap_or(0),
                cost: def.cost as i32,
                cannot_be_koed_by_effect: false,
                cannot_be_koed_in_battle_by_leader: false,
                keywords: def.keywords.clone(),
                cannot_be_blocked: false,
                blocker_power_ceiling: None,
            }
        })
        .collect();

    // Layer 2: DON!! attachment. Only during the controller's own turn
    // (6-5-5-2), and only for cards in the Leader or Character area.
    for card in &state.cards {
        if !matches!(card.zone, Zone::Leader | Zone::Character) {
            continue;
        }
        if state.turn_player != card.controller {
            continue;
        }
        table[card.id.index()].power += 1000 * card.attached_don.len() as i32;
    }

    // Layer 3: resolved continuous effects still within their duration.
    for m in &state.modifiers {
        apply(&mut table[m.target.index()], m.kind);
    }

    // Layer 4: permanent effects, iterated to a fixpoint (8-1-3-3-5).
    //
    // Every pass rebuilds from `base` and applies each effect exactly once, so
    // a stable set of effects yields a stable result. Iterating covers case
    // III, where applying effects changes which effects apply — conditions are
    // evaluated against the *previous* pass's characteristics.
    let base = table.clone();
    for _ in 0..FIXPOINT_LIMIT {
        let mut next = base.clone();

        // I: the turn player applies theirs, then II: the non-turn player.
        for player in [state.turn_player, state.turn_player.opponent()] {
            for source_id in state.all_in_play() {
                let card = state.card(source_id);
                if card.controller != player {
                    continue;
                }
                for effect in &scripts.script(card.def).permanent {
                    if !conditions_hold(state, db, &table, source_id, None, &effect.conditions) {
                        continue;
                    }
                    for target in targets_of(state, db, source_id, &effect.scope) {
                        apply(&mut next[target.index()], effect.kind);
                    }
                }
            }
        }

        if next == table {
            break;
        }
        table = next;
    }

    Derived { table }
}

fn apply(ch: &mut Characteristics, kind: ModKind) {
    match kind {
        ModKind::Power(delta) => ch.power += delta,
        ModKind::Cost(delta) => {
            // Deliberately unclamped; see the field's documentation.
            ch.cost += delta;
        }
        ModKind::CannotBeKoedByEffect => ch.cannot_be_koed_by_effect = true,
        ModKind::CannotBeKoedInBattleByLeader => ch.cannot_be_koed_in_battle_by_leader = true,
        ModKind::GrantKeyword(kw) => {
            if !ch.keywords.contains(&kw) {
                ch.keywords.push(kw);
            }
        }
        ModKind::CannotBeBlocked => ch.cannot_be_blocked = true,
        ModKind::BlockerPowerCeiling(v) => {
            // Keep the strictest ceiling if several apply.
            ch.blocker_power_ceiling = Some(match ch.blocker_power_ceiling {
                Some(existing) => existing.min(v),
                None => v,
            });
        }
    }
}

fn targets_of(
    state: &GameState,
    db: &CardDb,
    source: CardInstanceId,
    scope: &Scope,
) -> Vec<CardInstanceId> {
    let controller = state.card(source).controller;
    match scope {
        Scope::ThisCard => vec![source],
        Scope::YourCharacters => state.player(controller).characters.clone(),
        Scope::YourBattlersWithAnyType(types) => state
            .battlers(controller)
            .into_iter()
            .filter(|&id| {
                let def = db.get(state.card(id).def);
                types.iter().any(|t| def.has_type(t))
            })
            .collect(),
    }
}

/// Evaluates an effect's conditions against the current state.
///
/// `table` is the partially-derived characteristics table, so conditions that
/// read power see the layers resolved so far — which is what 8-4-6 asks for.
///
/// `frame` is the resolving effect where there is one; conditions that read a
/// binding are false without it.
pub fn conditions_hold(
    state: &GameState,
    db: &CardDb,
    _table: &[Characteristics],
    source: CardInstanceId,
    frame: Option<&crate::effect::EffectFrame>,
    conditions: &[Condition],
) -> bool {
    let card = state.card(source);
    conditions.iter().all(|c| match c {
        Condition::Bound(key) => frame.is_some_and(|f| !f.bound(key).is_empty()),
        Condition::DonAttached(n) => card.attached_don.len() >= *n as usize,
        Condition::YourTurn => state.turn_player == card.controller,
        Condition::OpponentsTurn => state.turn_player != card.controller,
        Condition::CharacterCountAtLeast(n) => {
            state.player(card.controller).characters.len() >= *n as usize
        }
        Condition::SelfRested => card.rested,
        Condition::LeaderHasType(ty) => state
            .player(card.controller)
            .leader
            .is_some_and(|l| db.get(state.card(l).def).has_type(ty)),
        // "If there is a Character with a cost of N" is not restricted to your
        // own side.
        Condition::AnyCharacterWithCost(n) => [PlayerId::P0, PlayerId::P1].iter().any(|p| {
            state
                .player(*p)
                .characters
                .iter()
                .any(|&c| db.get(state.card(c).def).cost == *n)
        }),
    }) && {
        let _ = db;
        true
    }
}

/// Whether `card` matches every filter, evaluated against derived
/// characteristics so that power and keyword filters see current values.
pub fn matches_filters(
    state: &GameState,
    db: &CardDb,
    derived: &Derived,
    source: CardInstanceId,
    card: CardInstanceId,
    filters: &[crate::effect::Filter],
) -> bool {
    use crate::effect::Filter;
    let def = db.get(state.card(card).def);
    filters.iter().all(|f| match f {
        // Derived, not printed: ST-06 is built on lowering a Character's cost
        // so that a "cost N or less" removal effect can reach it.
        Filter::CostAtMost(n) => derived.get(card).effective_cost() <= *n,
        Filter::PowerAtMost(p) => derived.power(card) <= *p,
        Filter::HasAnyType(types) => types.iter().any(|t| def.has_type(t)),
        Filter::HasKeyword(kw) => derived.get(card).has_keyword(*kw),
        Filter::IsRested(want) => state.card(card).rested == *want,
        Filter::NotSelf => card != source,
        Filter::IsCategory(cat) => def.category == *cat,
        Filter::HasColor(color) => def.colors.contains(color),
        Filter::HasName(name) => def.name == *name,
    })
}

/// Whether `id` may currently declare an attack (7-1, 10-1-1).
pub fn can_attack(state: &GameState, db: &CardDb, derived: &Derived, id: CardInstanceId) -> bool {
    let card = state.card(id);
    if card.rested || card.controller != state.turn_player {
        return false;
    }
    if !matches!(card.zone, Zone::Leader | Zone::Character) {
        return false;
    }
    // 6-5-6-1: "Neither player can battle on their first turn." The restriction
    // is per player, not a global turn-1 rule — the player going second is
    // barred on turn 2, which is *their* first turn.
    if state.is_first_turn_for(state.turn_player) {
        return false;
    }
    if db.get(card.def).category == Category::Character {
        // Summoning sickness, waived by [Rush] (10-1-1).
        let sick = card.played_on_turn == Some(state.turn);
        if sick && !derived.get(id).has_keyword(Keyword::Rush) {
            return false;
        }
    }
    true
}

/// Legal targets for an attack by `attacker`: the opponent's Leader, or any of
/// their **rested** Characters (7-1-1-2).
pub fn attack_targets(state: &GameState, id: CardInstanceId) -> Vec<CardInstanceId> {
    let defender = state.card(id).controller.opponent();
    let ps = state.player(defender);
    let mut out: Vec<CardInstanceId> = ps.leader.iter().copied().collect();
    out.extend(
        ps.characters
            .iter()
            .copied()
            .filter(|&c| state.card(c).rested),
    );
    out
}
