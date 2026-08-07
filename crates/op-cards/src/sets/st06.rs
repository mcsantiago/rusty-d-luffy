//! ST-06 — Absolute Justice.
//!
//! The deck's plan is a loop: reduce an opponent Character's cost to 0, then
//! remove it with an effect that only reaches cost 0. Sakazuki K.O.s "a
//! Character with a cost of 0", and nothing is *printed* at cost 0 — the
//! target is manufactured by Sengoku, Hina, Helmeppo and Tashigi first.
//!
//! Two things make that work, and both are easy to get wrong. `CostAtMost`
//! reads *derived* cost, so a reduction actually changes what removal can
//! reach. And a reduction past 0 is not an error: -4 on a 3-cost Character
//! still leaves a legal target, because a negative cost reads as 0 (1-3).
//!
//! Vanilla cards (ST06-003, -009, -011, -013) and keyword-only cards
//! (ST06-007 `[Blocker]`) need no entry.

use op_core::card::Keyword;
use op_core::script::CardScript;

use crate::dsl::*;

pub fn scripts() -> Vec<(&'static str, CardScript)> {
    vec![
        // [Activate: Main] [Once Per Turn] ③ You may trash 1 card from your
        // hand: K.O. up to 1 of your opponent's Characters with a cost of 0.
        (
            "ST06-001",
            Script::new()
                .activated(activated_once(
                    cost(3, false, 1),
                    [ko_opponent_costing(0)].concat(),
                ))
                .build(),
        ),
        // [On Play] You may trash 1 card from your hand: K.O. up to 1 of your
        // opponent's Characters with a cost of 0.
        (
            "ST06-002",
            Script::new()
                .auto(auto_paying(
                    Timing::OnPlay,
                    cost(0, false, 1),
                    vec![],
                    ko_opponent_costing(0),
                ))
                .build(),
        ),
        // This Character cannot be K.O.'d by effects.
        // [DON!! x1] If there is a Character with a cost of 0, this Character
        // gains [Double Attack].
        (
            "ST06-004",
            Script::new()
                .permanent(permanent_self(vec![], ModKind::CannotBeKoedByEffect))
                .permanent(permanent_self(
                    vec![don(1), any_character_costing(0)],
                    ModKind::GrantKeyword(Keyword::DoubleAttack),
                ))
                .build(),
        ),
        // [When Attacking] Give up to 1 of your opponent's Characters −4 cost
        // during this turn.
        (
            "ST06-005",
            Script::new()
                .auto(auto(Timing::WhenAttacking, vec![], cost_down(4)))
                .build(),
        ),
        // [Activate: Main] You may rest this Character: Give up to 1 of your
        // opponent's Characters −2 cost during this turn.
        (
            "ST06-006",
            Script::new()
                .activated(activated(cost(0, true, 0), cost_down(2)))
                .build(),
        ),
        // [On Play] Give up to 1 of your opponent's Characters −4 cost.
        (
            "ST06-008",
            Script::new()
                .auto(auto(Timing::OnPlay, vec![], cost_down(4)))
                .build(),
        ),
        // [On Play] Give up to 1 of your opponent's Characters −3 cost.
        (
            "ST06-010",
            Script::new()
                .auto(auto(Timing::OnPlay, vec![], cost_down(3)))
                .build(),
        ),
        // [Activate: Main] You may trash 1 card from your hand and rest this
        // Character: K.O. up to 1 of your opponent's Characters with a cost of
        // 4 or less.
        (
            "ST06-012",
            Script::new()
                .activated(activated(cost(0, true, 1), ko_opponent_costing(4)))
                .build(),
        ),
        // [Counter] Up to 1 of your Leader or Character cards gains +4000 power
        // during this battle. Then, K.O. up to 1 of your opponent's active
        // Characters with a cost of 3 or less.
        (
            "ST06-014",
            Script::new()
                .counter(vec![
                    power_up(TARGET, 4000, ThisBattle),
                    choose(
                        "k",
                        filtered(opponent_characters(1), vec![cost_at_most(3), rested(false)]),
                    ),
                    ko("k"),
                ])
                .build(),
        ),
        // [Main] Draw 1 card. Then, give up to 1 of your opponent's Characters
        // −2 cost during this turn.
        (
            "ST06-015",
            Script::new()
                .activated(activated(free(), [vec![draw(1)], cost_down(2)].concat()))
                .build(),
        ),
        // [Counter] Up to 1 of your Leader or Character cards gains +2000 power
        // during this battle.
        (
            "ST06-016",
            Script::new()
                .counter(vec![power_up(TARGET, 2000, ThisBattle)])
                .build(),
        ),
        // [On Play] Give up to 1 of your opponent's Characters −1 cost.
        // [Activate: Main] You may rest this Stage: If your Leader has the
        // {Navy} type, give up to 1 of your opponent's Characters −1 cost.
        (
            "ST06-017",
            Script::new()
                .auto(auto(Timing::OnPlay, vec![], cost_down(1)))
                .activated(activated(
                    cost(0, true, 0),
                    [vec![require_if(leader_has_type("Navy"))], cost_down(1)].concat(),
                ))
                .build(),
        ),
    ]
}

/// "Give up to 1 of your opponent's Characters −N cost during this turn."
fn cost_down(amount: i32) -> Vec<op_core::effect::EffectOp> {
    vec![
        choose("c", opponent_characters(1)),
        modify("c", ModKind::Cost(-amount), ThisTurn),
    ]
}

/// "K.O. up to 1 of your opponent's Characters with a cost of N or less."
///
/// Reads derived cost, so it reaches Characters this deck has just shrunk.
fn ko_opponent_costing(max: u8) -> Vec<op_core::effect::EffectOp> {
    vec![
        choose(
            "k",
            filtered(opponent_characters(1), vec![cost_at_most(max)]),
        ),
        ko("k"),
    ]
}
