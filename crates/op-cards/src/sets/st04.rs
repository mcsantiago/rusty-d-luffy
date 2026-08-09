//! ST-04 — Animal Kingdom Pirates.
//!
//! Purple, and the whole set is about the DON!! deck. Almost every card either
//! spends DON!! permanently ("DON!! −N": return that many from your field to
//! your DON!! deck) or refills early ("add up to 1 DON!! card from your DON!!
//! deck"). Both are new kinds of resource movement: `rest_don` turns DON!!
//! sideways and gets it back next Refresh Phase, whereas "DON!! −N" gives it up
//! for good unless it comes back around off the deck.
//!
//! That makes the set's tempo the reverse of every deck implemented so far. It
//! ramps ahead of the DON!! curve and then pays the ramp back to remove things,
//! so a turn's effective DON!! count is no longer simply "the turn number".
//!
//! Vanilla cards (ST04-007, -009, -012, -013) and keyword-only cards (ST04-011
//! `[Blocker]`) need no entry.

use op_core::card::Keyword;
use op_core::script::CardScript;

use crate::dsl::*;

pub fn scripts() -> Vec<(&'static str, CardScript)> {
    vec![
        // [Activate: Main] [Once Per Turn] DON!! −7: Trash up to 1 of your
        // opponent's Life cards.
        //
        // Life is a secret area (3-1-4), so the top card is taken rather than
        // offering a pick between face-down cards — see `EffectOp::TrashLife`.
        (
            "ST04-001",
            Script::new()
                .activated(activated_once(don_minus(7), vec![trash_opponent_life(1)]))
                .build(),
        ),
        // [On Play] DON!! −1: Play up to 1 [Page One] card with a cost of 4 or
        // less from your hand.
        //
        // By name, not card number: "[Page One]" reaches every printing of it
        // (2-14-3). ST04-012 is the only one in this set.
        (
            "ST04-002",
            Script::new()
                .auto(auto_paying(
                    Timing::OnPlay,
                    don_minus(1),
                    vec![],
                    vec![
                        choose(
                            "p",
                            filtered(your_hand(1), vec![named("Page One"), cost_at_most(4)]),
                        ),
                        play_bound("p"),
                    ],
                ))
                .build(),
        ),
        // [On Play] DON!! −5: K.O. up to 1 of your opponent's Characters with a
        // cost of 6 or less. This Character gains [Rush] during this turn.
        (
            "ST04-003",
            Script::new()
                .auto(auto_paying(
                    Timing::OnPlay,
                    don_minus(5),
                    vec![],
                    [
                        ko_opponent_costing(6),
                        vec![modify(THIS, ModKind::GrantKeyword(Keyword::Rush), ThisTurn)],
                    ]
                    .concat(),
                ))
                .build(),
        ),
        // [On Play] DON!! −1: K.O. up to 1 of your opponent's Characters with a
        // cost of 4 or less.
        (
            "ST04-004",
            Script::new()
                .auto(auto_paying(
                    Timing::OnPlay,
                    don_minus(1),
                    vec![],
                    ko_opponent_costing(4),
                ))
                .build(),
        ),
        // [Blocker] is printed. [On Play] DON!! −1: Draw 2 cards and trash 1
        // card from your hand.
        //
        // The trash is an instruction, not an offer, so the choice is mandatory
        // — and it comes after the draw, which guarantees it a legal target.
        (
            "ST04-005",
            Script::new()
                .auto(auto_paying(
                    Timing::OnPlay,
                    don_minus(1),
                    vec![],
                    vec![draw(2), choose("d", exactly(your_hand(1), 1)), trash("d")],
                ))
                .build(),
        ),
        // [On Play] DON!! −1: Draw 1 card.
        (
            "ST04-006",
            Script::new()
                .auto(auto_paying(
                    Timing::OnPlay,
                    don_minus(1),
                    vec![],
                    vec![draw(1)],
                ))
                .build(),
        ),
        // [On Play] You may trash 1 card from your hand: Add up to 1 DON!! card
        // from your DON!! deck and set it as active.
        (
            "ST04-008",
            Script::new()
                .auto(auto_paying(
                    Timing::OnPlay,
                    cost(0, false, 1),
                    vec![],
                    vec![add_don(1, false)],
                ))
                .build(),
        ),
        // [On Play] DON!! −1: K.O. up to 1 of your opponent's Characters with a
        // cost of 3 or less.
        // [Trigger] Play this card.
        (
            "ST04-010",
            Script::new()
                .auto(auto_paying(
                    Timing::OnPlay,
                    don_minus(1),
                    vec![],
                    ko_opponent_costing(3),
                ))
                .trigger(vec![play_bound(THIS)])
                .build(),
        ),
        // [Main] Draw 1 card, then add up to 1 DON!! card from your DON!! deck
        // and set it as active.
        // [Trigger] Activate this card's [Main] effect.
        (
            "ST04-014",
            Script::new()
                .activated(activated(free(), vec![draw(1), add_don(1, false)]))
                .trigger(vec![draw(1), add_don(1, false)])
                .build(),
        ),
        // [Main] K.O. up to 1 of your opponent's Characters with a cost of 6 or
        // less, then add up to 1 DON!! card from your DON!! deck and set it as
        // active.
        // [Trigger] Add up to 1 DON!! card from your DON!! deck and set it as
        // active.
        (
            "ST04-015",
            Script::new()
                .activated(activated(
                    free(),
                    [ko_opponent_costing(6), vec![add_don(1, false)]].concat(),
                ))
                .trigger(vec![add_don(1, false)])
                .build(),
        ),
        // [Counter] DON!! −1: Up to 1 of your Leader or Character cards gains
        // +4000 power during this battle.
        //
        // The DON!! −1 is on top of the card's printed cost of 1, so countering
        // with it costs one DON!! rested and one returned to the DON!! deck.
        (
            "ST04-016",
            Script::new()
                .counter_paying(don_minus(1), vec![power_up(TARGET, 4000, ThisBattle)])
                .build(),
        ),
        // [Activate: Main] You may rest this Stage: If your Leader has the
        // {Animal Kingdom Pirates} type, add up to 1 DON!! card from your DON!!
        // deck and rest it.
        (
            "ST04-017",
            Script::new()
                .activated(activated(
                    cost(0, true, 0),
                    vec![
                        require_if(leader_has_type("Animal Kingdom Pirates")),
                        add_don(1, true),
                    ],
                ))
                .build(),
        ),
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
