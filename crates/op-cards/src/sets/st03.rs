//! ST-03 — The Seven Warlords of the Sea.
//!
//! Blue, and the set is built on returning Characters to hand. Five of its
//! seventeen cards bounce something, at every point on the curve and from every
//! kind of effect — a Leader activation, two `[On Play]`s, a `[Main]` Event and
//! a `[Counter]` Event.
//!
//! Bouncing is not removal. The card comes back, so the tempo is borrowed
//! rather than won, and the deck pairs it with card draw and a deck search to
//! stay ahead of the hand it keeps handing back. That makes it the first deck
//! here whose plan is disruption rather than board presence.
//!
//! Two things in the text are easy to read past. "Return … to the **owner's**
//! hand" is not the controller's — `MoveTo` already sends a card to its owner's
//! zone, so an opponent's Character goes home to them. And plain "Character"
//! with no "your" reaches both sides (`Who::Both`), which is what makes these
//! effects usable on your own board when that is the better line.
//!
//! Vanilla cards (ST03-002, -006, -011, -012) and keyword-only cards
//! (ST03-008, -013, both `[Blocker]`) need no entry.

use op_core::effect::Duration::ThisBattle;
use op_core::script::CardScript;

use crate::dsl::*;

pub fn scripts() -> Vec<(&'static str, CardScript)> {
    vec![
        // [Activate: Main] [Once Per Turn] DON!! −4: Return up to 1 Character
        // with a cost of 5 or less to the owner's hand.
        (
            "ST03-001",
            Script::new()
                .activated(activated_once(
                    don_minus(4),
                    vec![
                        choose("t", filtered(any_characters(1), vec![cost_at_most(5)])),
                        to_hand("t"),
                    ],
                ))
                .build(),
        ),
        // [Blocker]
        // [DON!! x1] [On Block] Place up to 1 Character with a cost of 2 or
        // less at the bottom of the owner's deck.
        //
        // [Blocker] is printed, so it comes off the card data; only the
        // [On Block] needs scripting. "At the bottom of the owner's deck" is
        // `MoveTo(Zone::Deck)`, which places at the bottom by definition.
        //
        // This is the card that made [On Block] worth wiring: bottoming the
        // attacker ends the battle before the Counter Step (7-1-2-3).
        (
            "ST03-003",
            Script::new()
                .auto(auto(
                    Timing::OnBlock,
                    vec![don(1)],
                    vec![
                        choose("t", filtered(any_characters(1), vec![cost_at_most(2)])),
                        to_deck_bottom("t"),
                    ],
                ))
                .build(),
        ),
        // [On Play] Add up to 1 {The Seven Warlords of the Sea} or {Thriller
        // Bark Pirates} type Character with a cost of 4 or less other than
        // [Gecko Moria] from your trash to your hand.
        //
        // The exclusion is by name, so it covers every printing of Gecko Moria
        // rather than this card number alone (2-14-3) — including this card
        // itself once it is in the trash.
        (
            "ST03-004",
            Script::new()
                .auto(auto(
                    Timing::OnPlay,
                    vec![],
                    vec![
                        choose(
                            "t",
                            filtered(
                                your_trash(1),
                                vec![
                                    of_type(&[
                                        "The Seven Warlords of the Sea",
                                        "Thriller Bark Pirates",
                                    ]),
                                    is_character(),
                                    cost_at_most(4),
                                    not(named("Gecko Moria")),
                                ],
                            ),
                        ),
                        to_hand("t"),
                    ],
                ))
                .build(),
        ),
        // [DON!! x1] [When Attacking] Draw 2 cards and trash 2 cards from your
        // hand.
        //
        // Trashing is an instruction rather than an offer, so the selection
        // carries a floor: the player picks which two, but not whether.
        (
            "ST03-005",
            Script::new()
                .auto(auto(
                    Timing::WhenAttacking,
                    vec![don(1)],
                    vec![draw(2), choose("d", exactly(your_hand(2), 2)), trash("d")],
                ))
                .build(),
        ),
        // [DON!! x1] [Activate: Main] [Once Per Turn] ➁: Play up to 1
        // [Pacifista] with a cost of 4 or less from your deck, then shuffle
        // your deck.
        //
        // The shuffle is not decoration: without it the player would know the
        // order of everything they just looked through.
        //
        // The [DON!! x1] is a condition, not a cost — the DON!! must be
        // attached for the effect to be offered, and activating does not spend
        // it. The ➁ is the cost.
        (
            "ST03-007",
            Script::new()
                .activated(activated_once_when(
                    vec![don(1)],
                    cost(2, false, 0),
                    vec![
                        choose(
                            "p",
                            filtered(your_deck(1), vec![named("Pacifista"), cost_at_most(4)]),
                        ),
                        play_bound("p"),
                        shuffle_your_deck(),
                    ],
                ))
                .build(),
        ),
        // [On Play] Return up to 1 Character with a cost of 7 or less to the
        // owner's hand.
        (
            "ST03-009",
            Script::new()
                .auto(auto(
                    Timing::OnPlay,
                    vec![],
                    vec![
                        choose("t", filtered(any_characters(1), vec![cost_at_most(7)])),
                        to_hand("t"),
                    ],
                ))
                .build(),
        ),
        // [On Play] Look at 3 cards from the top of your deck and return them
        // to the top or bottom of the deck in any order.
        // [Trigger] Play this card.
        //
        // The order is the whole effect, so it is the player's: see
        // `EffectOp::LookTop`. Playing it off the [Trigger] fires the [On Play]
        // as well, which is where the look comes from.
        (
            "ST03-010",
            Script::new()
                .auto(auto(Timing::OnPlay, vec![], vec![look_top(3, "l")]))
                .trigger(vec![play_bound(THIS)])
                .build(),
        ),
        // [Blocker]
        // [Trigger] Play this card.
        //
        // [Blocker] is printed, so only the [Trigger] needs scripting — which
        // is why this card cannot be `KEYWORD_ONLY` despite reading like its
        // neighbour ST03-008, whose text really is the keyword alone.
        (
            "ST03-013",
            Script::new().trigger(vec![play_bound(THIS)]).build(),
        ),
        // [On Play] Return up to 1 Character with a cost of 3 or less to the
        // owner's hand.
        (
            "ST03-014",
            Script::new()
                .auto(auto(
                    Timing::OnPlay,
                    vec![],
                    vec![
                        choose("t", filtered(any_characters(1), vec![cost_at_most(3)])),
                        to_hand("t"),
                    ],
                ))
                .build(),
        ),
        // [Main] Return up to 1 Character with a cost of 7 or less to the
        // owner's hand.
        // [Trigger] Activate this card's [Main] effect.
        //
        // The [Trigger] repeats the ops rather than referring to the effect,
        // which is how ST01-015 and ST04-014 express the same wording: a
        // `[Trigger]` is resolved from the Life area and has no activated
        // effect of its own to call.
        (
            "ST03-015",
            Script::new()
                .activated(activated(
                    free(),
                    vec![
                        choose("t", filtered(any_characters(1), vec![cost_at_most(7)])),
                        to_hand("t"),
                    ],
                ))
                .trigger(vec![
                    choose("t", filtered(any_characters(1), vec![cost_at_most(7)])),
                    to_hand("t"),
                ])
                .build(),
        ),
        // [Counter] Return up to 1 Character with a cost of 3 or less to the
        // owner's hand.
        // [Trigger] Activate this card's [Counter] effect.
        //
        // The [Counter] picks its own target rather than reading the engine's
        // TARGET binding, so the same ops are correct off a [Trigger], where no
        // battle is under way to have a target at all.
        (
            "ST03-016",
            Script::new()
                .counter(vec![
                    choose("t", filtered(any_characters(1), vec![cost_at_most(3)])),
                    to_hand("t"),
                ])
                .trigger(vec![
                    choose("t", filtered(any_characters(1), vec![cost_at_most(3)])),
                    to_hand("t"),
                ])
                .build(),
        ),
        // [Counter] Up to 1 of your Leader or Character cards gains +4000 power
        // during this battle. Then, draw 1 card if you have 3 or less cards in
        // your hand.
        //
        // The draw is conditional on the hand *after* the Counter has left it,
        // which is why the condition sits mid-effect in a `require_if` rather
        // than on the effect itself: an effect's conditions are read before it
        // resolves, and this one is asked afterwards.
        (
            "ST03-017",
            Script::new()
                .counter(vec![
                    power_up(TARGET, 4000, ThisBattle),
                    require_if(hand_at_most(3)),
                    draw(1),
                ])
                .build(),
        ),
    ]
}
