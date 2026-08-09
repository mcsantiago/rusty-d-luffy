//! ST-08 — Monkey D. Luffy.
//!
//! Black, and built on the same axis as ST-06: shrink an opponent's Character
//! until a "cost N or less" removal effect can reach it. Where ST-06 shrinks to
//! 0 and removes with a cost-0 effect, ST-08 shrinks hard (−7 on ST08-014) and
//! removes with the wider cost-2 effects the deck is full of. Both rely on
//! `CostAtMost` reading *derived* cost.
//!
//! The Leader turns that removal into resources: every Character K.O.'d on your
//! turn, yours or theirs, hands it a rested DON!! card.
//!
//! Vanilla cards (ST08-003, -010, -011, -012) and keyword-only cards (ST08-007
//! `[Blocker]`, whose `[Trigger]` is still scripted below) need no entry for
//! their body text.

use op_core::card::Color;
use op_core::script::CardScript;

use crate::dsl::*;

pub fn scripts() -> Vec<(&'static str, CardScript)> {
    vec![
        // [Your Turn] When a Character is K.O.'d, give up to 1 rested DON!!
        // card to this Leader.
        //
        // "a Character" is either player's, and the deck K.O.s plenty of its
        // own, so this fires on both sides of the board.
        (
            "ST08-001",
            Script::new()
                .auto(auto(
                    Timing::OnCharacterKoed,
                    vec![your_turn()],
                    vec![give_don(THIS, 1, DonSource::Rested)],
                ))
                .build(),
        ),
        // This Character cannot be K.O.'d in battle by Leaders.
        // [Activate: Main] You may rest this Character: Give up to 1 of your
        // opponent's Characters −2 cost during this turn.
        (
            "ST08-002",
            Script::new()
                .permanent(permanent_self(
                    vec![],
                    ModKind::CannotBeKoedInBattleByLeader,
                ))
                .activated(activated(cost(0, true, 0), cost_down(2)))
                .build(),
        ),
        // [Activate: Main] You may rest this Character: K.O. up to 1 of your
        // opponent's Characters with a cost of 2 or less.
        (
            "ST08-004",
            Script::new()
                .activated(activated(cost(0, true, 0), ko_opponent_costing(2)))
                .build(),
        ),
        // [On Play] You may trash 1 card from your hand: K.O. all Characters
        // with a cost of 1 or less.
        //
        // "all Characters" is both boards, this card's own side included — and
        // it is not a choice, so it binds every match at once rather than
        // offering the player a subset.
        (
            "ST08-005",
            Script::new()
                .auto(auto_paying(
                    Timing::OnPlay,
                    cost(0, false, 1),
                    vec![],
                    vec![
                        select_all("k", filtered(all_characters(), vec![cost_at_most(1)])),
                        ko("k"),
                    ],
                ))
                .build(),
        ),
        // [Blocker] is printed. [On Play] Give up to 1 of your opponent's
        // Characters −4 cost during this turn.
        (
            "ST08-006",
            Script::new()
                .auto(auto(Timing::OnPlay, vec![], cost_down(4)))
                .build(),
        ),
        // [Blocker] is printed.
        // [Trigger] Play this card.
        (
            "ST08-007",
            Script::new().trigger(vec![play_bound(THIS)]).build(),
        ),
        // [On Play] Give up to 1 of your opponent's Characters −2 cost during
        // this turn.
        (
            "ST08-008",
            Script::new()
                .auto(auto(Timing::OnPlay, vec![], cost_down(2)))
                .build(),
        ),
        // [On Play] If there is a Character with a cost of 0, draw 1 card.
        //
        // Nothing in the pool is printed at cost 0, so this pays off only after
        // the deck's own reductions have landed — the same dependency ST-06's
        // Sakazuki has.
        (
            "ST08-009",
            Script::new()
                .auto(auto(
                    Timing::OnPlay,
                    vec![],
                    vec![require_if(any_character_costing(0)), draw(1)],
                ))
                .build(),
        ),
        // [DON!! x1] At the end of a battle in which this Character battles
        // your opponent's Character, you may K.O. the opponent's Character you
        // battled with. If you do, K.O. this Character.
        //
        // The "you may" is the choice itself: `BATTLED` holds exactly the one
        // card this Character fought, and an "up to 1" pick may be answered
        // with nothing (8-4-4-1). `require_bound` then gates the self-K.O. on
        // having taken it.
        //
        // Only reachable when neither Character died in the battle — 7-1-4-2,
        // the attacker losing — which is the trade the card is offering.
        //
        // Diverges in one corner: if the chosen Character turns out to be
        // protected from effect K.O. (10-2-1-1), the trade still costs this
        // card. Nothing in the implemented pool can produce that board.
        (
            "ST08-013",
            Script::new()
                .auto(auto(
                    Timing::EndOfBattle,
                    vec![don(1)],
                    vec![
                        choose_from("k", BATTLED, 1),
                        ko("k"),
                        require_bound("k"),
                        ko(THIS),
                    ],
                ))
                .build(),
        ),
        // [Main] You may add 1 card from the top of your Life cards to your
        // hand: Give up to 1 of your opponent's Characters −7 cost during this
        // turn.
        // [Trigger] Add up to 1 black Character card with a cost of 2 or less
        // from your trash to your hand.
        //
        // −7 is the deck's deepest reduction, and the only one that reliably
        // opens up ST08-009's cost-0 clause.
        (
            "ST08-014",
            Script::new()
                .activated(activated(life_cost(1), cost_down(7)))
                .trigger(vec![
                    choose(
                        "r",
                        filtered(
                            your_trash(1),
                            vec![is_character(), of_color(Color::Black), cost_at_most(2)],
                        ),
                    ),
                    to_hand("r"),
                ])
                .build(),
        ),
        // [Main] K.O. up to 1 of your opponent's Characters with a cost of 2 or
        // less.
        // [Trigger] Draw 1 card.
        (
            "ST08-015",
            Script::new()
                .activated(activated(free(), ko_opponent_costing(2)))
                .trigger(vec![draw(1)])
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
fn ko_opponent_costing(max: u8) -> Vec<op_core::effect::EffectOp> {
    vec![
        choose(
            "k",
            filtered(opponent_characters(1), vec![cost_at_most(max)]),
        ),
        ko("k"),
    ]
}
