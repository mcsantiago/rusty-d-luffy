//! ST-02 — Worst Generation.
//!
//! Vanilla cards (ST02-002, -006, -011, -012) and keyword-only cards
//! (ST02-004 `[Blocker]`) need no entry.

use op_core::script::CardScript;

use crate::dsl::*;

pub fn scripts() -> Vec<(&'static str, CardScript)> {
    vec![
        // [Activate: Main] [Once Per Turn] ③ You may trash 1 card from your
        // hand: Set this Leader as active.
        (
            "ST02-001",
            Script::new()
                .activated(activated_once(cost(3, false, 1), vec![set_active(THIS)]))
                .build(),
        ),
        // [DON!! x1] If you have 3 or more Characters, this card gains +2000
        // power.
        (
            "ST02-003",
            Script::new()
                .permanent(permanent_self(
                    vec![don(1), characters_at_least(3)],
                    ModKind::Power(2000),
                ))
                .build(),
        ),
        // [On Play] K.O. up to 1 of your opponent's rested Characters with a
        // cost of 3 or less.
        // [Trigger] Play this card.
        (
            "ST02-005",
            Script::new()
                .auto(auto(
                    Timing::OnPlay,
                    vec![],
                    vec![
                        choose(
                            "t",
                            filtered(
                                opponent_characters(1),
                                vec![rested(true), cost_at_most(3)],
                            ),
                        ),
                        ko("t"),
                    ],
                ))
                .trigger(vec![play_bound(THIS)])
                .build(),
        ),
        // [Activate: Main] ➀ You may rest this Character: Look at 5 cards from
        // the top of your deck; reveal up to 1 {Supernovas} type card and add
        // it to your hand. Then, place the rest at the bottom of your deck in
        // any order.
        (
            "ST02-007",
            Script::new()
                .activated(activated(
                    cost(1, true, 0),
                    vec![dig_top(5, "t", 1, vec![of_type(&["Supernovas"])])],
                ))
                .build(),
        ),
        // [DON!! x1] [When Attacking] Rest up to 1 of your opponent's DON!!
        // cards.
        (
            "ST02-008",
            Script::new()
                .auto(auto(
                    Timing::WhenAttacking,
                    vec![don(1)],
                    vec![
                        choose("d", filtered(opponent_don(1), vec![rested(false)])),
                        rest("d"),
                    ],
                ))
                .build(),
        ),
        // [On Play] Set up to 1 of your {Supernovas} or {Heart Pirates} type
        // rested Characters with a cost of 5 or less as active.
        (
            "ST02-009",
            Script::new()
                .auto(auto(
                    Timing::OnPlay,
                    vec![],
                    vec![
                        choose(
                            "t",
                            filtered(
                                your_characters(1),
                                vec![
                                    of_type(&["Supernovas", "Heart Pirates"]),
                                    rested(true),
                                    cost_at_most(5),
                                ],
                            ),
                        ),
                        set_active("t"),
                    ],
                ))
                .build(),
        ),
        // [DON!! x1] [Once Per Turn] [Your Turn] If this Character battles your
        // opponent's Character, set this card as active.
        //
        // "battles your opponent's Character" resolves at the end of a battle
        // whose target was a Character, which is the engine's EndOfBattle
        // timing.
        (
            "ST02-010",
            Script::new()
                .auto(auto_once(
                    Timing::EndOfBattle,
                    vec![don(1), your_turn()],
                    vec![set_active(THIS)],
                ))
                .build(),
        ),
        // [Blocker] is printed. [DON!! x1] [End of Your Turn] Set this
        // Character as active.
        (
            "ST02-013",
            Script::new()
                .auto(auto(
                    Timing::EndOfYourTurn,
                    vec![don(1)],
                    vec![set_active(THIS)],
                ))
                .build(),
        ),
        // [DON!! x1] [Your Turn] If this Character is rested, your {Supernovas}
        // or {Navy} type Leaders and Characters gain +1000 power.
        (
            "ST02-014",
            Script::new()
                .permanent(permanent_typed(
                    vec![don(1), your_turn(), self_rested()],
                    &["Supernovas", "Navy"],
                    ModKind::Power(1000),
                ))
                .build(),
        ),
        // [Counter] Up to 1 of your Leader or Character cards gains +2000 power
        // during this battle. Then, set up to 1 of your DON!! cards as active.
        // [Trigger] Set up to 2 of your DON!! cards as active.
        (
            "ST02-015",
            Script::new()
                .counter(vec![
                    power_up(TARGET, 2000, ThisBattle),
                    choose("d", filtered(your_don(1), vec![rested(true)])),
                    set_active("d"),
                ])
                .trigger(vec![
                    choose("d", filtered(your_don(2), vec![rested(true)])),
                    set_active("d"),
                ])
                .build(),
        ),
        // [Counter] Up to 1 of your Leader or Character cards gains +4000 power
        // during this battle. Then, set up to 1 of your DON!! cards as active.
        (
            "ST02-016",
            Script::new()
                .counter(vec![
                    power_up(TARGET, 4000, ThisBattle),
                    choose("d", filtered(your_don(1), vec![rested(true)])),
                    set_active("d"),
                ])
                .build(),
        ),
        // [Main] Rest up to 1 of your opponent's Characters.
        // [Trigger] Play up to 1 {Supernovas} type card with a cost of 2 or
        // less from your hand.
        (
            "ST02-017",
            Script::new()
                .activated(activated(
                    free(),
                    vec![
                        choose("t", filtered(opponent_characters(1), vec![rested(false)])),
                        rest("t"),
                    ],
                ))
                .trigger(vec![
                    choose(
                        "t",
                        filtered(
                            your_hand(1),
                            vec![
                                of_type(&["Supernovas"]),
                                cost_at_most(2),
                                is_character(),
                            ],
                        ),
                    ),
                    play_bound("t"),
                ])
                .build(),
        ),
    ]
}
