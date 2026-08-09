//! ST-01 — Straw Hat Crew.
//!
//! Each entry pairs a card number with its printed text and the script that
//! encodes it. Vanilla cards (ST01-003, -008, -009, -010) and cards whose only
//! text is a printed keyword (ST01-006 `[Blocker]`) need no entry.

use op_core::card::Keyword;
use op_core::script::CardScript;

use crate::dsl::*;

pub fn scripts() -> Vec<(&'static str, CardScript)> {
    vec![
        // [Activate: Main] [Once Per Turn] Give this Leader or 1 of your
        // Characters up to 1 rested DON!! card.
        (
            "ST01-001",
            Script::new()
                .activated(activated_once(
                    free(),
                    vec![
                        choose("t", your_battlers(1)),
                        give_don("t", 1, DonSource::Rested),
                    ],
                ))
                .build(),
        ),
        // [DON!! x2] [When Attacking] Your opponent cannot activate a [Blocker]
        // Character that has 5000 or more power during this battle.
        // [Trigger] Play this card.
        (
            "ST01-002",
            Script::new()
                .auto(auto(
                    Timing::WhenAttacking,
                    vec![don(2)],
                    vec![blocker_ceiling(THIS, 5000, ThisBattle)],
                ))
                .trigger(vec![play_bound(THIS)])
                .build(),
        ),
        // [DON!! x2] This Character gains [Rush].
        (
            "ST01-004",
            Script::new()
                .permanent(permanent_self(
                    vec![don(2)],
                    ModKind::GrantKeyword(Keyword::Rush),
                ))
                .build(),
        ),
        // [DON!! x1] [When Attacking] Up to 1 of your Leader or Character cards
        // other than this card gains +1000 power during this turn.
        (
            "ST01-005",
            Script::new()
                .auto(auto(
                    Timing::WhenAttacking,
                    vec![don(1)],
                    vec![
                        choose("t", filtered(your_battlers(1), vec![other_than_self()])),
                        power_up("t", 1000, ThisTurn),
                    ],
                ))
                .build(),
        ),
        // [Activate: Main] [Once Per Turn] Give up to 1 rested DON!! card to
        // your Leader or 1 of your Characters.
        (
            "ST01-007",
            Script::new()
                .activated(activated_once(
                    free(),
                    vec![
                        choose("t", your_battlers(1)),
                        give_don("t", 1, DonSource::Rested),
                    ],
                ))
                .build(),
        ),
        // [On Play] Give up to 2 rested DON!! cards to your Leader or 1 of your
        // Characters.
        (
            "ST01-011",
            Script::new()
                .auto(auto(
                    Timing::OnPlay,
                    vec![],
                    vec![
                        choose("t", your_battlers(1)),
                        give_don("t", 2, DonSource::Rested),
                    ],
                ))
                .build(),
        ),
        // [Rush] is printed. [DON!! x2] [When Attacking] Your opponent cannot
        // activate [Blocker] during this battle.
        (
            "ST01-012",
            Script::new()
                .auto(auto(
                    Timing::WhenAttacking,
                    vec![don(2)],
                    vec![cannot_be_blocked(THIS, ThisBattle)],
                ))
                .build(),
        ),
        // [DON!! x1] This Character gains +1000 power.
        (
            "ST01-013",
            Script::new()
                .permanent(permanent_self(vec![don(1)], ModKind::Power(1000)))
                .build(),
        ),
        // [Counter] Up to 1 of your Leader or Character cards gains +3000 power
        // during this battle.
        // [Trigger] Up to 1 of your Leader or Character cards gains +1000 power
        // during this turn.
        (
            "ST01-014",
            Script::new()
                // The boosted card is chosen when the Counter is declared, so
                // it arrives pre-bound rather than through a Choose op.
                .counter(vec![power_up(TARGET, 3000, ThisBattle)])
                .trigger(vec![
                    choose("t", your_battlers(1)),
                    power_up("t", 1000, ThisTurn),
                ])
                .build(),
        ),
        // [Main] K.O. up to 1 of your opponent's Characters with 6000 power or
        // less.
        // [Trigger] Activate this card's [Main] effect.
        (
            "ST01-015",
            Script::new()
                .activated(activated(
                    free(),
                    vec![
                        choose(
                            "t",
                            filtered(opponent_characters(1), vec![power_at_most(6000)]),
                        ),
                        ko("t"),
                    ],
                ))
                .trigger(vec![
                    choose(
                        "t",
                        filtered(opponent_characters(1), vec![power_at_most(6000)]),
                    ),
                    ko("t"),
                ])
                .build(),
        ),
        // [Main] Select up to 1 of your {Straw Hat Crew} type Leader or
        // Character cards. Your opponent cannot activate [Blocker] if that
        // Leader or Character attacks during this turn.
        // [Trigger] K.O. up to 1 of your opponent's [Blocker] Characters with a
        // cost of 3 or less.
        (
            "ST01-016",
            Script::new()
                .activated(activated(
                    free(),
                    vec![
                        choose(
                            "t",
                            filtered(your_battlers(1), vec![of_type(&["Straw Hat Crew"])]),
                        ),
                        cannot_be_blocked("t", ThisTurn),
                    ],
                ))
                .trigger(vec![
                    choose(
                        "t",
                        filtered(
                            opponent_characters(1),
                            vec![with_keyword(Keyword::Blocker), cost_at_most(3)],
                        ),
                    ),
                    ko("t"),
                ])
                .build(),
        ),
        // [Activate: Main] You may rest this Stage: Up to 1 {Straw Hat Crew}
        // type Leader or Character card on your field gains +1000 power during
        // this turn.
        (
            "ST01-017",
            Script::new()
                .activated(activated(
                    cost(0, true, 0),
                    vec![
                        choose(
                            "t",
                            filtered(your_battlers(1), vec![of_type(&["Straw Hat Crew"])]),
                        ),
                        power_up("t", 1000, ThisTurn),
                    ],
                ))
                .build(),
        ),
    ]
}
