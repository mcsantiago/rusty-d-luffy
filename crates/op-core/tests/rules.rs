//! Rules conformance suite.
//!
//! Tests are named for the Comprehensive Rules v1.2.0 clause they pin down, so
//! that a failure points straight at the rule text it violates.

mod common;

use common::{deck_of, game_with, TestCards, TestScripts};
use op_core::effect::{DonSource, EffectOp, Timing, Who, SELF_BINDING};
use op_core::script::{ActivationCost, AutoEffect, CardScript};
use op_core::state::Placement;
use op_core::zone::Zone;
use op_core::{Action, BattleStep, Game, GameEvent, GameOver, Pending, Phase, PlayerId};

/// Drives the game to the turn player's Main Phase, taking the default answer
/// to any setup decision along the way.
fn to_main(game: &mut Game) {
    for _ in 0..64 {
        match game.pending() {
            Some(Pending::Mulligan { .. }) => {
                game.step(Action::Mulligan(false)).unwrap();
            }
            Some(Pending::MainAction { .. }) => return,
            Some(other) => panic!("unexpected pending during setup: {other:?}"),
            None => panic!("game parked with no pending decision"),
        }
    }
    panic!("never reached a Main Phase");
}

/// Ends the current turn and runs to the next Main Phase.
fn end_turn(game: &mut Game) {
    game.step(Action::EndMainPhase).unwrap();
    to_main(game);
}

fn fixture() -> (TestCards, Game) {
    let cards = TestCards::new();
    let (game, _) = game_with(
        &cards,
        TestScripts::default(),
        7,
        ("LDR-001", deck_of("CHR-5K", 30)),
        ("LDR-002", deck_of("CHR-5K", 30)),
    );
    (cards, game)
}

// ---- setup -----------------------------------------------------------------

#[test]
fn rule_5_2_1_6_opening_hand_is_five_cards() {
    let (_cards, game) = fixture();
    assert_eq!(game.state.player(PlayerId::P0).hand.len(), 5);
    assert_eq!(game.state.player(PlayerId::P1).hand.len(), 5);
}

#[test]
fn rule_5_2_1_7_life_equals_leader_life_value() {
    let (_cards, mut game) = fixture();
    to_main(&mut game);
    // LDR-001 has 5 Life, LDR-002 has 4.
    assert_eq!(game.state.player(PlayerId::P0).life.len(), 5);
    assert_eq!(game.state.player(PlayerId::P1).life.len(), 4);
}

#[test]
fn rule_5_2_1_7_life_is_placed_top_of_deck_to_bottom_of_life() {
    let (_cards, mut game) = fixture();
    // Capture deck order before Life is placed.
    let deck_before: Vec<_> = game.state.player(PlayerId::P0).deck[..5].to_vec();
    to_main(&mut game);
    let life = &game.state.player(PlayerId::P0).life;
    // The card that was on top of the deck ends up at the bottom of Life, so
    // the first card taken as damage is the *fifth* card off the deck.
    assert_eq!(life.last(), deck_before.first());
    assert_eq!(life.first(), deck_before.get(4));
}

#[test]
fn rule_6_3_1_first_player_skips_their_first_draw() {
    let (_cards, mut game) = fixture();
    to_main(&mut game);
    assert_eq!(game.state.turn, 1);
    assert_eq!(game.state.turn_player, PlayerId::P0);
    // Still 5: the opening hand, with no draw-phase card added.
    assert_eq!(game.state.player(PlayerId::P0).hand.len(), 5);
}

#[test]
fn rule_6_4_1_first_player_places_one_don_on_turn_one_then_two() {
    let (_cards, mut game) = fixture();
    to_main(&mut game);
    assert_eq!(game.state.player(PlayerId::P0).cost_area.len(), 1);

    end_turn(&mut game); // P1's turn 2
    assert_eq!(game.state.player(PlayerId::P1).cost_area.len(), 2);

    end_turn(&mut game); // back to P0, turn 3
    assert_eq!(game.state.player(PlayerId::P0).cost_area.len(), 3);
}

/// 6-5-6-1: "Neither player can battle on their first turn."
///
/// The restriction is per player, so it covers turn 1 *and* turn 2 — turn 2
/// being the second player's own first turn. Both seats are exercised here
/// because checking only the first player leaves the more easily broken half
/// of the rule untested.
#[test]
fn rule_6_5_6_1_neither_player_can_battle_on_their_own_first_turn() {
    let (_cards, mut game) = fixture();
    to_main(&mut game);

    // Turn 1 — the first player's first turn.
    assert_eq!(game.state.turn, 1);
    assert_eq!(game.state.turn_player, PlayerId::P0);
    let p0_leader = game.state.player(PlayerId::P0).leader.unwrap();
    let p1_leader = game.state.player(PlayerId::P1).leader.unwrap();
    assert!(
        game.step(Action::Attack {
            attacker: p0_leader,
            target: p1_leader
        })
        .is_err(),
        "the first player must not battle on turn 1"
    );

    // Turn 2 — the second player's first turn.
    end_turn(&mut game);
    assert_eq!(game.state.turn, 2);
    assert_eq!(game.state.turn_player, PlayerId::P1);
    assert!(
        game.step(Action::Attack {
            attacker: p1_leader,
            target: p0_leader
        })
        .is_err(),
        "the second player must not battle on turn 2, which is their first turn"
    );

    // Turn 3 — the first player's second turn, so battles are legal.
    end_turn(&mut game);
    assert_eq!(game.state.turn, 3);
    assert!(
        game.step(Action::Attack {
            attacker: p0_leader,
            target: p1_leader
        })
        .is_ok(),
        "battles must be legal from turn 3"
    );
}

/// The legal-action generator must agree with the restriction, or a search
/// agent would consider attacks that the engine then rejects.
#[test]
fn rule_6_5_6_1_first_turns_offer_no_attack_actions() {
    let (_cards, mut game) = fixture();
    to_main(&mut game);

    for expected_turn in [1u32, 2] {
        assert_eq!(game.state.turn, expected_turn);
        assert!(
            !op_core::legal_actions(&game)
                .iter()
                .any(|a| matches!(a, Action::Attack { .. })),
            "turn {expected_turn} offered an attack"
        );
        end_turn(&mut game);
    }

    assert_eq!(game.state.turn, 3);
    assert!(
        op_core::legal_actions(&game)
            .iter()
            .any(|a| matches!(a, Action::Attack { .. })),
        "turn 3 should offer attacks"
    );
}

// ---- DON!! -----------------------------------------------------------------

#[test]
fn rule_6_5_5_2_given_don_grants_1000_power_on_your_turn_only() {
    let (_cards, mut game) = fixture();
    to_main(&mut game);
    let leader = game.state.player(PlayerId::P0).leader.unwrap();
    assert_eq!(game.derived().power(leader), 5000);

    game.step(Action::GiveDon { to: leader }).unwrap();
    assert_eq!(game.derived().power(leader), 6000);

    // 6-5-5-2 limits the bonus to the controller's own turn.
    end_turn(&mut game);
    assert_eq!(game.state.turn_player, PlayerId::P1);
    assert_eq!(game.derived().power(leader), 5000);
}

#[test]
fn rule_6_2_3_refresh_returns_given_don_rested_to_the_cost_area() {
    let (_cards, mut game) = fixture();
    to_main(&mut game);
    let leader = game.state.player(PlayerId::P0).leader.unwrap();
    game.step(Action::GiveDon { to: leader }).unwrap();
    assert_eq!(game.state.card(leader).attached_don.len(), 1);

    end_turn(&mut game); // P1 turn
    end_turn(&mut game); // back to P0: Refresh runs

    assert!(game.state.card(leader).attached_don.is_empty());
    // Returned DON!! are set active again by 6-2-4 in the same phase.
    assert_eq!(game.state.player(PlayerId::P0).cost_area.len(), 3);
    assert_eq!(game.active_don(PlayerId::P0).len(), 3);
}

/// 6-1-1: a turn is a Refresh, Draw, DON!!, Main and End Phase, in that order.
///
/// This pins the *announcement*, not the behaviour. The End Phase does real,
/// rules-visible work — 6-6-1-1 queues the end-of-turn autos and 6-6-1-3
/// expires "during this turn" modifiers — but it used to enter without pushing
/// `PhaseStarted`, because Main→End happens on the player's word in
/// `main_action` rather than in `tick_phase`. A trace could then not tell a
/// quiet End Phase from one that never ran.
#[test]
fn rule_6_1_1_every_phase_of_a_turn_announces_itself() {
    let (_cards, mut game) = fixture();
    to_main(&mut game);

    // Turn 2 rather than turn 1: its Refresh announcement arrives in the same
    // step that ends turn 1, so one window holds all five of its phases.
    let mut events = game.step(Action::EndMainPhase).unwrap().events;
    events.extend(game.step(Action::EndMainPhase).unwrap().events);

    let turn_2: Vec<_> = events
        .iter()
        .skip_while(|e| !matches!(e, GameEvent::TurnStarted { turn: 2, .. }))
        .take_while(|e| !matches!(e, GameEvent::TurnStarted { turn: 3, .. }))
        .filter_map(|e| match *e {
            GameEvent::PhaseStarted { phase, player } => Some((phase, player)),
            _ => None,
        })
        .collect();

    assert_eq!(
        turn_2,
        vec![
            (Phase::Refresh, PlayerId::P1),
            (Phase::Draw, PlayerId::P1),
            (Phase::Don, PlayerId::P1),
            (Phase::Main, PlayerId::P1),
            (Phase::End, PlayerId::P1),
        ]
    );
}

// ---- giving DON!! ----------------------------------------------------------

/// A card whose activated effect gives `n` DON!! to itself, drawn per `source`.
/// Bound to the effect's own card so the give is not preceded by a choice.
fn gives_don_to_self(source: DonSource, n: u8) -> op_core::script::CardScript {
    op_core::script::CardScript {
        activated: vec![op_core::script::ActivatedEffect {
            conditions: vec![],
            cost: op_core::script::ActivationCost::default(),
            ops: vec![op_core::effect::EffectOp::GiveDon {
                key: op_core::effect::SELF_BINDING.to_string(),
                n,
                source,
            }],
            slot: 0,
            once_per_turn: false,
        }],
        ..Default::default()
    }
}

/// Sets up turn 3, where the turn player holds three DON!!, with a source card
/// in play whose effect gives to itself from `source`.
fn don_fixture(source: DonSource, n: u8) -> (Game, op_core::CardInstanceId) {
    let cards = TestCards::new();
    let scripts = TestScripts::default().with(cards.def("CHR-5K"), gives_don_to_self(source, n));
    let (mut game, _) = game_with(
        &cards,
        scripts,
        7,
        ("LDR-001", deck_of("CHR-5K", 30)),
        ("LDR-002", deck_of("CHR-5K", 30)),
    );
    to_main(&mut game);
    end_turn(&mut game);
    end_turn(&mut game);
    assert_eq!(game.state.player(PlayerId::P0).cost_area.len(), 3);
    let source = put_in_play(&mut game, PlayerId::P0, "CHR-5K");
    (game, source)
}

/// 6-5-5-1: "Place 1 **active** DON!! card from your cost area underneath your
/// Leader or a Character card". The ordinary give takes an active DON!!, and
/// giving does not rest it — 6-2-3 does that on its way back.
#[test]
fn rule_6_5_5_1_the_ordinary_give_takes_an_active_don_and_leaves_it_active() {
    let (_cards, mut game) = fixture();
    to_main(&mut game);
    let leader = game.state.player(PlayerId::P0).leader.unwrap();

    // Turn 1 gives the first player exactly one DON!!. Rest it, and the give
    // has nothing to take.
    let don = game.state.player(PlayerId::P0).cost_area[0];
    game.state.card_mut(don).rested = true;
    assert!(
        !op_core::legal_actions(&game).contains(&Action::GiveDon { to: leader }),
        "a rested DON!! is not an active one (6-5-5-1)"
    );
    assert!(game.step(Action::GiveDon { to: leader }).is_err());

    game.state.card_mut(don).rested = false;
    game.step(Action::GiveDon { to: leader }).unwrap();
    assert_eq!(game.state.card(leader).attached_don, vec![don]);
    assert!(
        game.state.card(don).is_active(),
        "giving is not resting; the DON!! keeps the state it was in"
    );
}

/// ST01-001: "Give this Leader or 1 of your Characters up to 1 rested DON!!
/// card." The adjective qualifies the DON!! being *selected*, not the state it
/// ends up in — Bandai's ruling refuses a DON!! already given to another
/// Character on exactly that ground. An active DON!! is therefore not
/// available to the effect, however much the player would like to spend it.
#[test]
fn a_rested_don_source_takes_the_rested_don_and_not_an_active_one() {
    let (mut game, source) = don_fixture(DonSource::Rested, 1);

    let don = game.state.player(PlayerId::P0).cost_area.clone();
    game.state.card_mut(don[2]).rested = true;

    game.step(Action::ActivateEffect {
        card: source,
        slot: 0,
        discard: vec![],
    })
    .unwrap();

    assert_eq!(
        game.state.card(source).attached_don,
        vec![don[2]],
        "the rested DON!! is the only one the effect may take"
    );
    assert!(
        game.state.card(don[2]).rested,
        "it was already rested and giving does not change that"
    );
    for &active in &don[..2] {
        assert!(
            game.state.card(active).is_active(),
            "an untaken DON!! is left alone"
        );
    }
}

/// The same effect with nothing rested to take. It gives nothing because the
/// pool is empty, not because anyone chose zero: the engine takes `n` greedily
/// and never offers the count as a decision, so "up to" is honoured here only
/// by coincidence. (4-8-1 and 8-4-4-1 say the player picks between 0 and n;
/// that gap is pre-existing and tracked separately.) The failure this guards
/// against is the engine helping itself to an active DON!! instead.
#[test]
fn a_rested_don_source_gives_nothing_when_every_don_is_active() {
    let (mut game, source) = don_fixture(DonSource::Rested, 1);
    let before = game.state.player(PlayerId::P0).cost_area.clone();

    game.step(Action::ActivateEffect {
        card: source,
        slot: 0,
        discard: vec![],
    })
    .unwrap();

    assert!(game.state.card(source).attached_don.is_empty());
    assert_eq!(
        game.state.player(PlayerId::P0).cost_area,
        before,
        "no DON!! left the cost area"
    );
}

/// `n` is a ceiling, not a quota: two rested DON!! satisfy a give of two, and
/// one rested DON!! gives one rather than reaching for an active one.
#[test]
fn a_rested_don_source_gives_as_many_as_are_rested_up_to_n() {
    let (mut game, source) = don_fixture(DonSource::Rested, 2);
    let don = game.state.player(PlayerId::P0).cost_area.clone();
    game.state.card_mut(don[1]).rested = true;

    game.step(Action::ActivateEffect {
        card: source,
        slot: 0,
        discard: vec![],
    })
    .unwrap();

    assert_eq!(game.state.card(source).attached_don, vec![don[1]]);
}

/// 8-4-4-1 lets a player choose fewer only where the text offers a choice.
/// A selector whose floor is the whole pool offers none — "K.O. **all**
/// Characters with a cost of 1 or less" is an instruction — so the engine binds
/// every candidate without staging a decision.
///
/// This pins the *mechanism*, not the outcome: a version that parked on
/// `Pending::Choose` with a single legal subset would K.O. the same cards and
/// pass any board-level assertion, while handing the search a pointless subset
/// enumeration and the player a question with one answer.
#[test]
fn rule_8_4_4_1_a_choice_with_no_discretion_is_not_offered_as_one() {
    use op_core::effect::{EffectOp, Selector, Who, ALL};

    let cards = TestCards::new();
    let ko_every_character = op_core::script::CardScript {
        activated: vec![op_core::script::ActivatedEffect {
            conditions: vec![],
            cost: op_core::script::ActivationCost::default(),
            ops: vec![
                EffectOp::Choose {
                    key: "k".to_string(),
                    select: Selector {
                        zone: Zone::Character,
                        owner: Who::Opponent,
                        from: None,
                        up_to: ALL,
                        at_least: ALL,
                        filters: vec![],
                    },
                },
                EffectOp::Ko {
                    key: "k".to_string(),
                },
            ],
            slot: 0,
            once_per_turn: false,
        }],
        ..Default::default()
    };
    let scripts = TestScripts::default().with(cards.def("CHR-5K"), ko_every_character);
    let (mut game, _) = game_with(
        &cards,
        scripts,
        7,
        ("LDR-001", deck_of("CHR-5K", 30)),
        ("LDR-002", deck_of("CHR-5K", 30)),
    );
    to_main(&mut game);

    let source = put_in_play(&mut game, PlayerId::P0, "CHR-5K");
    let victims = [
        put_in_play(&mut game, PlayerId::P1, "CHR-5K"),
        put_in_play(&mut game, PlayerId::P1, "CHR-5K"),
    ];

    let out = game
        .step(Action::ActivateEffect {
            card: source,
            slot: 0,
            discard: vec![],
        })
        .unwrap();

    assert!(
        !matches!(out.pending, Some(Pending::Choose { .. })),
        "a choice with one legal answer must not be staged as a decision"
    );
    for victim in victims {
        assert_eq!(
            game.state.card(victim).zone,
            Zone::Trash,
            "every candidate should have been taken"
        );
    }
}

/// The "If you do," half of "you may X. If you do, Y." (8-3-3): a condition
/// that reads the frame's own bindings, so declining the optional half stops
/// the consequence. Outside a resolving effect there is no frame, and it reads
/// false rather than panicking.
#[test]
fn rule_8_3_3_a_condition_can_read_whether_the_player_took_the_optional_half() {
    use op_core::effect::{Condition, EffectOp, Selector, Who};

    let cards = TestCards::new();
    let may_ko_then_draw = op_core::script::CardScript {
        activated: vec![op_core::script::ActivatedEffect {
            conditions: vec![],
            cost: op_core::script::ActivationCost::default(),
            ops: vec![
                // "You may K.O. up to 1 of your opponent's Characters."
                EffectOp::Choose {
                    key: "k".to_string(),
                    select: Selector {
                        zone: Zone::Character,
                        owner: Who::Opponent,
                        from: None,
                        up_to: 1,
                        at_least: 0,
                        filters: vec![],
                    },
                },
                EffectOp::Ko {
                    key: "k".to_string(),
                },
                // "If you do, draw 1."
                EffectOp::RequireIf {
                    cond: Condition::Bound("k".to_string()),
                },
                EffectOp::Draw {
                    player: Who::You,
                    n: 1,
                },
            ],
            slot: 0,
            once_per_turn: false,
        }],
        ..Default::default()
    };
    let scripts = TestScripts::default().with(cards.def("CHR-5K"), may_ko_then_draw);
    let (mut game, _) = game_with(
        &cards,
        scripts,
        7,
        ("LDR-001", deck_of("CHR-5K", 30)),
        ("LDR-002", deck_of("CHR-5K", 30)),
    );
    to_main(&mut game);

    let source = put_in_play(&mut game, PlayerId::P0, "CHR-5K");
    put_in_play(&mut game, PlayerId::P1, "CHR-5K");

    let hand_before = game.state.player(PlayerId::P0).hand.len();
    game.step(Action::ActivateEffect {
        card: source,
        slot: 0,
        discard: vec![],
    })
    .unwrap();

    // Decline: bind nothing, so the draw must not happen.
    game.step(Action::Choose { cards: vec![] }).unwrap();
    assert_eq!(
        game.state.player(PlayerId::P0).hand.len(),
        hand_before,
        "declining the optional half must stop the consequence"
    );
}

// ---- battle ----------------------------------------------------------------

/// Declines every block and counter offered, accumulating the events, and
/// returns whatever decision is pending afterwards. Tests that care about the
/// damage step use this so they are not derailed by a defender who merely
/// happens to hold cards with a Counter value.
fn decline_defenses(
    game: &mut Game,
    out: op_core::StepOutcome,
) -> (Vec<GameEvent>, Option<Pending>) {
    let mut events = out.events;
    let mut pending = out.pending;
    loop {
        match pending {
            Some(Pending::Block { .. }) => {
                let o = game.step(Action::Block { blocker: None }).unwrap();
                events.extend(o.events);
                pending = o.pending;
            }
            Some(Pending::Counter { .. }) => {
                let o = game.step(Action::DoneCountering).unwrap();
                events.extend(o.events);
                pending = o.pending;
            }
            other => return (events, other),
        }
    }
}

/// Forces `card` into `player`'s Character area, bypassing cost. Used to set up
/// board states that would otherwise take many turns to reach.
fn put_in_play(game: &mut Game, player: PlayerId, number: &str) -> op_core::CardInstanceId {
    let def = game.db().by_number(number).unwrap();
    let card = game.state.spawn(def, player, Zone::Limbo);
    game.state
        .move_card(card, player, Zone::Character, Placement::Bottom);
    card
}

/// Appends a card to `player`'s hand, so a test can state the hand order it
/// depends on rather than inheriting whatever the shuffle dealt.
fn put_in_hand(game: &mut Game, player: PlayerId, number: &str) -> op_core::CardInstanceId {
    let def = game.db().by_number(number).unwrap();
    let card = game.state.spawn(def, player, Zone::Limbo);
    game.state
        .move_card(card, player, Zone::Hand, Placement::Bottom);
    card
}

#[test]
fn rule_7_1_4_1_attacker_wins_ties_and_kos_the_character() {
    let cards = TestCards::new();
    let (mut game, _) = game_with(
        &cards,
        TestScripts::default(),
        7,
        ("LDR-001", deck_of("CHR-5K", 30)),
        ("LDR-002", deck_of("CHR-5K", 30)),
    );
    to_main(&mut game);
    end_turn(&mut game); // turn 2, P1
    end_turn(&mut game); // turn 3, P0 — battles are legal now

    let attacker = put_in_play(&mut game, PlayerId::P0, "CHR-5K");
    let defender = put_in_play(&mut game, PlayerId::P1, "CHR-5K");
    // Only rested Characters may be attacked (7-1-1-2).
    game.state.card_mut(defender).rested = true;
    // Clear summoning sickness so the attacker may declare.
    game.state.card_mut(attacker).played_on_turn = None;

    let out = game
        .step(Action::Attack {
            attacker,
            target: defender,
        })
        .unwrap();
    let (events, _) = decline_defenses(&mut game, out);

    // Equal power: the attacker wins (7-1-4-1) and the target is K.O.'d.
    assert!(events
        .iter()
        .any(|e| matches!(e, GameEvent::KnockedOut { card } if *card == defender)));
    assert_eq!(game.state.card(defender).zone, Zone::Trash);
}

#[test]
fn rule_7_1_4_2_weaker_attacker_achieves_nothing() {
    let cards = TestCards::new();
    let (mut game, _) = game_with(
        &cards,
        TestScripts::default(),
        7,
        ("LDR-001", deck_of("CHR-2K", 30)),
        ("LDR-002", deck_of("CHR-5K", 30)),
    );
    to_main(&mut game);
    end_turn(&mut game);
    end_turn(&mut game);

    let attacker = put_in_play(&mut game, PlayerId::P0, "CHR-2K");
    let defender = put_in_play(&mut game, PlayerId::P1, "CHR-5K");
    game.state.card_mut(defender).rested = true;
    game.state.card_mut(attacker).played_on_turn = None;

    let out = game
        .step(Action::Attack {
            attacker,
            target: defender,
        })
        .unwrap();
    let (events, _) = decline_defenses(&mut game, out);

    assert!(events.iter().any(|e| matches!(
        e,
        GameEvent::BattleResolved {
            attacker_won: false,
            ..
        }
    )));
    assert_eq!(game.state.card(defender).zone, Zone::Character);
}

#[test]
fn rule_7_1_1_2_only_rested_characters_can_be_attacked() {
    let cards = TestCards::new();
    let (mut game, _) = game_with(
        &cards,
        TestScripts::default(),
        7,
        ("LDR-001", deck_of("CHR-5K", 30)),
        ("LDR-002", deck_of("CHR-5K", 30)),
    );
    to_main(&mut game);
    end_turn(&mut game);
    end_turn(&mut game);

    let attacker = put_in_play(&mut game, PlayerId::P0, "CHR-5K");
    let active_defender = put_in_play(&mut game, PlayerId::P1, "CHR-5K");
    game.state.card_mut(attacker).played_on_turn = None;

    assert!(game
        .step(Action::Attack {
            attacker,
            target: active_defender
        })
        .is_err());
}

#[test]
fn rule_10_1_4_blocker_becomes_the_new_target() {
    let cards = TestCards::new();
    let (mut game, _) = game_with(
        &cards,
        TestScripts::default(),
        7,
        ("LDR-001", deck_of("CHR-7K", 30)),
        ("LDR-002", deck_of("CHR-BLOCK", 30)),
    );
    to_main(&mut game);
    end_turn(&mut game);
    end_turn(&mut game);

    let attacker = put_in_play(&mut game, PlayerId::P0, "CHR-7K");
    let blocker = put_in_play(&mut game, PlayerId::P1, "CHR-BLOCK");
    game.state.card_mut(attacker).played_on_turn = None;

    let enemy_leader = game.state.player(PlayerId::P1).leader.unwrap();
    let out = game
        .step(Action::Attack {
            attacker,
            target: enemy_leader,
        })
        .unwrap();

    // The defender is offered the block.
    assert!(matches!(out.pending, Some(Pending::Block { player }) if player == PlayerId::P1));

    let out = game
        .step(Action::Block {
            blocker: Some(blocker),
        })
        .unwrap();

    assert!(out
        .events
        .iter()
        .any(|e| matches!(e, GameEvent::Blocked { blocker: b, .. } if *b == blocker)));
    // 1000-power blocker eats a 7000-power attack and dies; the Leader is safe.
    assert_eq!(game.state.card(blocker).zone, Zone::Trash);
    assert_eq!(game.state.player(PlayerId::P1).life.len(), 4);
}

#[test]
fn rule_10_1_7_unblockable_skips_the_block_step() {
    let cards = TestCards::new();
    let (mut game, _) = game_with(
        &cards,
        TestScripts::default(),
        7,
        ("LDR-001", deck_of("CHR-UNBLOCK", 30)),
        ("LDR-002", deck_of("CHR-BLOCK", 30)),
    );
    to_main(&mut game);
    end_turn(&mut game);
    end_turn(&mut game);

    let attacker = put_in_play(&mut game, PlayerId::P0, "CHR-UNBLOCK");
    put_in_play(&mut game, PlayerId::P1, "CHR-BLOCK");
    game.state.card_mut(attacker).played_on_turn = None;

    let enemy_leader = game.state.player(PlayerId::P1).leader.unwrap();
    let out = game
        .step(Action::Attack {
            attacker,
            target: enemy_leader,
        })
        .unwrap();

    // No block offered; the defender only gets the Counter Step.
    assert!(!matches!(out.pending, Some(Pending::Block { .. })));
}

#[test]
fn rule_7_1_3_2_1_counter_from_hand_can_save_the_leader() {
    let cards = TestCards::new();
    let (mut game, _) = game_with(
        &cards,
        TestScripts::default(),
        7,
        ("LDR-001", deck_of("CHR-5K", 30)),
        // CHR-5K has a 1000 Counter; the defender will hold several.
        ("LDR-002", deck_of("CHR-5K", 30)),
    );
    to_main(&mut game);
    end_turn(&mut game);
    end_turn(&mut game);

    let attacker = put_in_play(&mut game, PlayerId::P0, "CHR-5K");
    game.state.card_mut(attacker).played_on_turn = None;
    let enemy_leader = game.state.player(PlayerId::P1).leader.unwrap();

    let out = game
        .step(Action::Attack {
            attacker,
            target: enemy_leader,
        })
        .unwrap();
    assert!(matches!(out.pending, Some(Pending::Counter { .. })));

    // 5000 attacker vs 5000 Leader: the attacker wins ties, so the defender
    // needs a Counter to survive.
    let counter_card = game.state.player(PlayerId::P1).hand[0];
    game.step(Action::Counter {
        card: counter_card,
        to: enemy_leader,
    })
    .unwrap();
    let out = game.step(Action::DoneCountering).unwrap();

    assert!(out.events.iter().any(|e| matches!(
        e,
        GameEvent::BattleResolved {
            attacker_won: false,
            target_power: 6000,
            ..
        }
    )));
    assert_eq!(game.state.player(PlayerId::P1).life.len(), 4);
}

/// 7-1-3-2-1: a printed Counter raises "their Leader or 1 Character card" —
/// the *player* chooses, it is not forced onto the card under attack. Multiple
/// Counters stack, and applied to the defending card they repel the attack.
#[test]
fn rule_7_1_3_2_1_counters_stack_on_the_chosen_card() {
    let cards = TestCards::new();
    let (mut game, _) = game_with(
        &cards,
        TestScripts::default(),
        7,
        ("LDR-001", deck_of("CHR-7K", 30)),
        ("LDR-002", deck_of("CHR-5K", 30)), // 1000 Counter each
    );
    to_main(&mut game);
    end_turn(&mut game);
    end_turn(&mut game);

    let attacker = put_in_play(&mut game, PlayerId::P0, "CHR-7K");
    game.state.card_mut(attacker).played_on_turn = None;
    let enemy_leader = game.state.player(PlayerId::P1).leader.unwrap();

    let out = game
        .step(Action::Attack {
            attacker,
            target: enemy_leader,
        })
        .unwrap();
    assert!(matches!(out.pending, Some(Pending::Counter { .. })));

    // 7000 attacker into a 5000 Leader: two 1000 Counters is not enough, three
    // is — the attacker winning ties means the defender must strictly exceed.
    let hand: Vec<_> = game.state.player(PlayerId::P1).hand.clone();
    for card in hand.iter().take(3) {
        game.step(Action::Counter {
            card: *card,
            to: enemy_leader,
        })
        .unwrap();
    }
    assert_eq!(game.derived().power(enemy_leader), 8000);

    let out = game.step(Action::DoneCountering).unwrap();
    assert!(
        out.events.iter().any(|e| matches!(
            e,
            GameEvent::BattleResolved {
                attacker_won: false,
                target_power: 8000,
                ..
            }
        )),
        "8000 defender repels a 7000 attacker"
    );
    assert_eq!(
        game.state.player(PlayerId::P1).life.len(),
        4,
        "no life is taken when the attack is repelled"
    );
}

/// The same Counters spent on a bystander are legal and achieve nothing.
///
/// This is the trap: the rules let you pick any of your Leader or Characters,
/// so an agent that does not model the battle will happily spend Counters
/// where they cannot matter.
#[test]
fn rule_7_1_3_2_1_a_counter_on_a_bystander_does_not_save_the_defender() {
    let cards = TestCards::new();
    let (mut game, _) = game_with(
        &cards,
        TestScripts::default(),
        7,
        ("LDR-001", deck_of("CHR-7K", 30)),
        ("LDR-002", deck_of("CHR-5K", 30)),
    );
    to_main(&mut game);
    end_turn(&mut game);
    end_turn(&mut game);

    let attacker = put_in_play(&mut game, PlayerId::P0, "CHR-7K");
    game.state.card_mut(attacker).played_on_turn = None;
    let bystander = put_in_play(&mut game, PlayerId::P1, "CHR-5K");
    let enemy_leader = game.state.player(PlayerId::P1).leader.unwrap();

    game.step(Action::Attack {
        attacker,
        target: enemy_leader,
    })
    .unwrap();

    // Legal — 7-1-3-2-1 says "their Leader or 1 Character card", not "the card
    // being attacked".
    let hand: Vec<_> = game.state.player(PlayerId::P1).hand.clone();
    for card in hand.iter().take(3) {
        game.step(Action::Counter {
            card: *card,
            to: bystander,
        })
        .unwrap();
    }
    assert_eq!(game.derived().power(bystander), 8000);
    assert_eq!(
        game.derived().power(enemy_leader),
        5000,
        "the card under attack is untouched"
    );

    let out = game.step(Action::DoneCountering).unwrap();
    assert!(out.events.iter().any(|e| matches!(
        e,
        GameEvent::BattleResolved {
            attacker_won: true,
            ..
        }
    )));
    assert_eq!(
        game.state.player(PlayerId::P1).life.len(),
        3,
        "three Counters were spent and a life was still lost"
    );
}

#[test]
fn rule_7_1_5_3_this_battle_modifiers_expire_at_end_of_battle() {
    let cards = TestCards::new();
    let (mut game, _) = game_with(
        &cards,
        TestScripts::default(),
        7,
        ("LDR-001", deck_of("CHR-2K", 30)),
        ("LDR-002", deck_of("CHR-5K", 30)),
    );
    to_main(&mut game);
    end_turn(&mut game);
    end_turn(&mut game);

    let attacker = put_in_play(&mut game, PlayerId::P0, "CHR-2K");
    game.state.card_mut(attacker).played_on_turn = None;
    let enemy_leader = game.state.player(PlayerId::P1).leader.unwrap();

    game.step(Action::Attack {
        attacker,
        target: enemy_leader,
    })
    .unwrap();
    let counter_card = game.state.player(PlayerId::P1).hand[0];
    game.step(Action::Counter {
        card: counter_card,
        to: enemy_leader,
    })
    .unwrap();
    assert_eq!(game.derived().power(enemy_leader), 6000);

    game.step(Action::DoneCountering).unwrap();
    assert!(game.state.battle.is_none());
    assert_eq!(game.derived().power(enemy_leader), 5000);
}

#[test]
fn rule_10_1_2_double_attack_takes_two_life() {
    let cards = TestCards::new();
    let (mut game, _) = game_with(
        &cards,
        TestScripts::default(),
        7,
        ("LDR-001", deck_of("CHR-DOUBLE", 30)),
        ("LDR-002", deck_of("CHR-7K", 30)),
    );
    to_main(&mut game);
    end_turn(&mut game);
    end_turn(&mut game);

    let attacker = put_in_play(&mut game, PlayerId::P0, "CHR-DOUBLE");
    game.state.card_mut(attacker).played_on_turn = None;
    let enemy_leader = game.state.player(PlayerId::P1).leader.unwrap();
    let life_before = game.state.player(PlayerId::P1).life.len();
    let hand_before = game.state.player(PlayerId::P1).hand.len();

    game.step(Action::Attack {
        attacker,
        target: enemy_leader,
    })
    .unwrap();
    // The defender has 7K bodies with no Counter value, so no Counter Step.
    assert_eq!(game.state.player(PlayerId::P1).life.len(), life_before - 2);
    assert_eq!(game.state.player(PlayerId::P1).hand.len(), hand_before + 2);
}

#[test]
fn rule_10_1_3_banish_trashes_the_life_card_instead_of_handing_it_over() {
    let cards = TestCards::new();
    let (mut game, _) = game_with(
        &cards,
        TestScripts::default(),
        7,
        ("LDR-001", deck_of("CHR-BANISH", 30)),
        ("LDR-002", deck_of("CHR-7K", 30)),
    );
    to_main(&mut game);
    end_turn(&mut game);
    end_turn(&mut game);

    let attacker = put_in_play(&mut game, PlayerId::P0, "CHR-BANISH");
    game.state.card_mut(attacker).played_on_turn = None;
    let enemy_leader = game.state.player(PlayerId::P1).leader.unwrap();
    let hand_before = game.state.player(PlayerId::P1).hand.len();
    let trash_before = game.state.player(PlayerId::P1).trash.len();

    let out = game
        .step(Action::Attack {
            attacker,
            target: enemy_leader,
        })
        .unwrap();

    assert!(out
        .events
        .iter()
        .any(|e| matches!(e, GameEvent::LifeTaken { banished: true, .. })));
    assert_eq!(game.state.player(PlayerId::P1).hand.len(), hand_before);
    assert_eq!(
        game.state.player(PlayerId::P1).trash.len(),
        trash_before + 1
    );
}

#[test]
fn rule_10_1_5_trigger_suspends_damage_and_offers_a_choice() {
    let cards = TestCards::new();
    // A Trigger is only offered when it has something to do; an unscripted
    // Trigger is treated as absent (see `Game::apply_one_damage`).
    let scripts = TestScripts::default().with(
        cards.def("CHR-TRIGGER"),
        op_core::script::CardScript {
            trigger: vec![op_core::effect::EffectOp::Draw {
                player: op_core::effect::Who::You,
                n: 1,
            }],
            ..Default::default()
        },
    );
    let (mut game, _) = game_with(
        &cards,
        scripts,
        7,
        ("LDR-001", deck_of("CHR-7K", 30)),
        ("LDR-002", deck_of("CHR-TRIGGER", 30)),
    );
    to_main(&mut game);
    end_turn(&mut game);
    end_turn(&mut game);

    let attacker = put_in_play(&mut game, PlayerId::P0, "CHR-7K");
    game.state.card_mut(attacker).played_on_turn = None;
    let enemy_leader = game.state.player(PlayerId::P1).leader.unwrap();

    let out = game
        .step(Action::Attack {
            attacker,
            target: enemy_leader,
        })
        .unwrap();
    let (_, pending) = decline_defenses(&mut game, out);

    // Every life card has a [Trigger], so damage suspends for the choice.
    assert!(
        matches!(pending, Some(Pending::Trigger { player, .. }) if player == PlayerId::P1),
        "expected a Trigger decision, got {pending:?}"
    );

    // Declining puts the card in hand as normal (10-1-5-2).
    let hand_before = game.state.player(PlayerId::P1).hand.len();
    game.step(Action::UseTrigger(false)).unwrap();
    assert_eq!(game.state.player(PlayerId::P1).hand.len(), hand_before + 1);
    assert!(game.state.battle.is_none());
}

#[test]
fn rule_7_1_2_1_only_one_blocker_per_battle() {
    let cards = TestCards::new();
    let (mut game, _) = game_with(
        &cards,
        TestScripts::default(),
        7,
        ("LDR-001", deck_of("CHR-7K", 30)),
        ("LDR-002", deck_of("CHR-BLOCK", 30)),
    );
    to_main(&mut game);
    end_turn(&mut game);
    end_turn(&mut game);

    let attacker = put_in_play(&mut game, PlayerId::P0, "CHR-7K");
    put_in_play(&mut game, PlayerId::P1, "CHR-BLOCK");
    put_in_play(&mut game, PlayerId::P1, "CHR-BLOCK");
    game.state.card_mut(attacker).played_on_turn = None;

    let enemy_leader = game.state.player(PlayerId::P1).leader.unwrap();
    game.step(Action::Attack {
        attacker,
        target: enemy_leader,
    })
    .unwrap();

    let first_blocker = game.legal_blockers()[0];
    game.step(Action::Block {
        blocker: Some(first_blocker),
    })
    .unwrap();

    // The battle is over; no second block was ever offered.
    assert!(game.state.battle.is_none());
}

// ---- rule processing -------------------------------------------------------

#[test]
fn rule_9_2_1_1_damage_at_zero_life_loses_the_game() {
    let cards = TestCards::new();
    let (mut game, _) = game_with(
        &cards,
        TestScripts::default(),
        7,
        ("LDR-001", deck_of("CHR-7K", 30)),
        ("LDR-002", deck_of("CHR-7K", 30)),
    );
    to_main(&mut game);
    end_turn(&mut game);
    end_turn(&mut game);

    // Empty P1's Life area, then swing at the Leader.
    let life: Vec<_> = game.state.player(PlayerId::P1).life.clone();
    for card in life {
        game.state
            .move_card(card, PlayerId::P1, Zone::Hand, Placement::Bottom);
    }
    assert!(game.state.player(PlayerId::P1).life.is_empty());

    let attacker = put_in_play(&mut game, PlayerId::P0, "CHR-7K");
    game.state.card_mut(attacker).played_on_turn = None;
    let enemy_leader = game.state.player(PlayerId::P1).leader.unwrap();

    let out = game
        .step(Action::Attack {
            attacker,
            target: enemy_leader,
        })
        .unwrap();

    assert_eq!(
        game.result(),
        Some(GameOver::LifeDepleted {
            loser: PlayerId::P1
        })
    );
    assert!(out
        .events
        .iter()
        .any(|e| matches!(e, GameEvent::GameEnded { .. })));
    assert_eq!(game.result().unwrap().winner(), Some(PlayerId::P0));
}

#[test]
fn rule_9_2_1_2_empty_deck_loses_the_game() {
    let cards = TestCards::new();
    let (mut game, _) = game_with(
        &cards,
        TestScripts::default(),
        7,
        ("LDR-001", deck_of("CHR-5K", 30)),
        ("LDR-002", deck_of("CHR-5K", 30)),
    );
    to_main(&mut game);

    // Drain P1's deck; rule processing fires on the next tick.
    let deck: Vec<_> = game.state.player(PlayerId::P1).deck.clone();
    for card in deck {
        game.state
            .move_card(card, PlayerId::P1, Zone::Trash, Placement::Top);
    }

    game.step(Action::EndMainPhase).unwrap();
    assert_eq!(
        game.result(),
        Some(GameOver::DeckOut {
            loser: PlayerId::P1
        })
    );
}

// ---- area limits -----------------------------------------------------------

/// 3-7-6-1: with five Characters out, a sixth is played by trashing one of
/// them first — the play is not refused, and *which* one is the player's
/// choice. The trash happens before the new card arrives, so its [On Play]
/// sees a board of four plus itself.
#[test]
fn rule_3_7_6_1_a_full_character_area_is_made_room_in() {
    let cards = TestCards::new();
    let (mut game, _) = game_with(
        &cards,
        TestScripts::default(),
        7,
        ("LDR-001", deck_of("CHR-2K", 30)),
        ("LDR-002", deck_of("CHR-2K", 30)),
    );
    to_main(&mut game);
    // Enough DON!! to afford the play.
    end_turn(&mut game);
    end_turn(&mut game);

    let mut board = Vec::new();
    for _ in 0..5 {
        board.push(put_in_play(&mut game, PlayerId::P0, "CHR-2K"));
    }
    assert_eq!(game.state.player(PlayerId::P0).characters.len(), 5);

    let in_hand = game.state.player(PlayerId::P0).hand[0];

    // Naming nothing is illegal — the rule requires making room.
    assert!(game
        .step(Action::PlayCard {
            card: in_hand,
            replacing: None
        })
        .is_err());

    // The generator offers one option per Character already out.
    let offers: Vec<_> = op_core::legal_actions(&game)
        .into_iter()
        .filter(|a| matches!(a, Action::PlayCard { card, .. } if *card == in_hand))
        .collect();
    assert_eq!(
        offers.len(),
        5,
        "one option per Character that could be trashed"
    );

    // Trashing someone else's Character is not making room on your own board.
    let theirs = put_in_play(&mut game, PlayerId::P1, "CHR-2K");
    assert!(game
        .step(Action::PlayCard {
            card: in_hand,
            replacing: Some(theirs)
        })
        .is_err());

    let victim = board[2];
    game.step(Action::PlayCard {
        card: in_hand,
        replacing: Some(victim),
    })
    .unwrap();

    assert_eq!(game.state.card(victim).zone, Zone::Trash);
    assert_eq!(game.state.card(in_hand).zone, Zone::Character);
    assert_eq!(
        game.state.player(PlayerId::P0).characters.len(),
        5,
        "still five: one left as one arrived"
    );
}

/// 8-3-1: a cost that trashes from hand is a choice, so every way of paying it
/// is offered rather than the engine picking for the player.
#[test]
fn rule_8_3_1_a_hand_cost_is_the_players_choice() {
    let cards = TestCards::new();
    let scripts = TestScripts::default().with(
        cards.def("CHR-5K"),
        op_core::script::CardScript {
            activated: vec![op_core::script::ActivatedEffect {
                conditions: vec![],
                cost: op_core::script::ActivationCost {
                    trash_from_hand: 1,
                    ..Default::default()
                },
                ops: vec![op_core::effect::EffectOp::Draw {
                    player: op_core::effect::Who::You,
                    n: 1,
                }],
                slot: 0,
                once_per_turn: false,
            }],
            ..Default::default()
        },
    );
    let (mut game, _) = game_with(
        &cards,
        scripts,
        7,
        ("LDR-001", deck_of("CHR-5K", 30)),
        ("LDR-002", deck_of("CHR-5K", 30)),
    );
    to_main(&mut game);
    let source = put_in_play(&mut game, PlayerId::P0, "CHR-5K");

    let hand = game.state.player(PlayerId::P0).hand.clone();
    let offers: Vec<_> = op_core::legal_actions(&game)
        .into_iter()
        .filter(|a| matches!(a, Action::ActivateEffect { .. }))
        .collect();
    assert_eq!(
        offers.len(),
        hand.len(),
        "one option per card that could be trashed"
    );

    // Naming the wrong number of cards is rejected rather than silently fixed.
    assert!(game
        .step(Action::ActivateEffect {
            card: source,
            slot: 0,
            discard: vec![]
        })
        .is_err());

    // The named card is the one that goes, not whichever came first.
    let chosen = hand[2];
    game.step(Action::ActivateEffect {
        card: source,
        slot: 0,
        discard: vec![chosen],
    })
    .unwrap();
    assert_eq!(game.state.card(chosen).zone, Zone::Trash);
    assert!(hand[0..2]
        .iter()
        .all(|&c| game.state.card(c).zone == Zone::Hand));
}

/// 8-3-1-3: a cost is paid in full or not at all — so one card cannot answer
/// for two. The second trip to the trash would find it already there, leaving
/// a two-card cost settled with one card and reported paid.
#[test]
fn rule_8_3_1_3_the_same_card_cannot_pay_a_hand_cost_twice() {
    let cards = TestCards::new();
    let scripts = TestScripts::default().with(
        cards.def("CHR-5K"),
        CardScript {
            activated: vec![op_core::script::ActivatedEffect {
                conditions: vec![],
                cost: ActivationCost {
                    trash_from_hand: 2,
                    ..Default::default()
                },
                ops: vec![EffectOp::Draw {
                    player: Who::You,
                    n: 1,
                }],
                slot: 0,
                once_per_turn: false,
            }],
            ..Default::default()
        },
    );
    let (mut game, _) = game_with(
        &cards,
        scripts,
        7,
        ("LDR-001", deck_of("CHR-5K", 30)),
        ("LDR-002", deck_of("CHR-5K", 30)),
    );
    to_main(&mut game);
    let source = put_in_play(&mut game, PlayerId::P0, "CHR-5K");

    let twice = game.state.player(PlayerId::P0).hand[0];
    assert!(game
        .step(Action::ActivateEffect {
            card: source,
            slot: 0,
            discard: vec![twice, twice],
        })
        .is_err());
    assert_eq!(
        game.state.card(twice).zone,
        Zone::Hand,
        "a refused activation spends nothing"
    );
}

/// 10-2-14-1: "trash" moves a card *selected from* the hand, and 8-4-4 leaves
/// the selection to the player — so an auto effect's cost asks, exactly as the
/// same cost named up front with an `ActivateEffect` does.
///
/// The hand is two cards with the one worth keeping on the left, because that
/// is the case a cost paying itself out of the front of the hand gets wrong
/// while a one-card hand hides it.
#[test]
fn rule_10_2_14_1_a_hand_cost_asks_which_card_to_trash() {
    let cards = TestCards::new();
    let scripts = TestScripts::default().with(
        cards.def("CHR-BLOCK"),
        CardScript {
            auto: vec![AutoEffect {
                timing: Timing::OnPlay,
                conditions: vec![],
                cost: ActivationCost {
                    trash_from_hand: 1,
                    ..Default::default()
                },
                ops: vec![EffectOp::Draw {
                    player: Who::You,
                    n: 1,
                }],
                slot: 0,
                once_per_turn: false,
            }],
            ..Default::default()
        },
    );
    let (mut game, _) = game_with(
        &cards,
        scripts,
        7,
        ("LDR-001", deck_of("CHR-5K", 30)),
        ("LDR-002", deck_of("CHR-5K", 30)),
    );
    to_main(&mut game);

    // A stated hand: the card being played, then the Counter card the player
    // needs, then the one they can spare.
    for card in game.state.player(PlayerId::P0).hand.clone() {
        game.state
            .move_card(card, PlayerId::P0, Zone::Deck, Placement::Bottom);
    }
    let source = put_in_hand(&mut game, PlayerId::P0, "CHR-BLOCK");
    let keeper = put_in_hand(&mut game, PlayerId::P0, "CHR-2K");
    let spare = put_in_hand(&mut game, PlayerId::P0, "CHR-7K");

    game.step(Action::PlayCard {
        card: source,
        replacing: None,
    })
    .unwrap();
    assert!(
        matches!(game.pending(), Some(Pending::PayCost { .. })),
        "the [On Play] cost is the controller's to decline first (8-3-1-4)"
    );

    let out = game.step(Action::PayCost(true)).unwrap();
    let Some(Pending::Choose {
        options,
        up_to,
        at_least,
        ..
    }) = out.pending
    else {
        panic!("agreeing to a hand cost should ask which card to trash");
    };
    assert_eq!(
        options,
        vec![keeper, spare],
        "every card in hand is eligible"
    );
    assert_eq!(
        (up_to, at_least),
        (1, 1),
        "the cost fixes the count; only which card is open"
    );
    assert_eq!(
        game.state.card(keeper).zone,
        Zone::Hand,
        "nothing may be spent before the player has answered"
    );

    // The generator offers both, so search and the RL mask see the decision too.
    assert_eq!(
        op_core::legal_actions(&game),
        vec![
            Action::Choose {
                cards: vec![keeper]
            },
            Action::Choose { cards: vec![spare] },
        ]
    );

    // Answering with nothing is refused: the cost is an instruction, not an
    // "up to" (8-4-4-1).
    assert!(game.step(Action::Choose { cards: vec![] }).is_err());
    // And a card that is not in hand cannot pay a cost taken from the hand.
    assert!(game
        .step(Action::Choose {
            cards: vec![source]
        })
        .is_err());

    game.step(Action::Choose { cards: vec![spare] }).unwrap();
    assert_eq!(game.state.card(spare).zone, Zone::Trash, "the card named");
    assert_eq!(
        game.state.card(keeper).zone,
        Zone::Hand,
        "and not whichever card happened to be first"
    );
}

// ---- battle step sequencing ------------------------------------------------

/// The battle steps a step outcome announced, in order.
fn battle_steps(out: &op_core::StepOutcome) -> Vec<BattleStep> {
    out.events
        .iter()
        .filter_map(|e| match e {
            GameEvent::BattleStepStarted { step } => Some(*step),
            _ => None,
        })
        .collect()
}

#[test]
fn rule_7_1_battle_runs_attack_block_counter_damage_end_in_order() {
    let cards = TestCards::new();
    let (mut game, _) = game_with(
        &cards,
        TestScripts::default(),
        7,
        ("LDR-001", deck_of("CHR-7K", 30)),
        ("LDR-002", deck_of("CHR-BLOCK", 30)),
    );
    to_main(&mut game);
    end_turn(&mut game);
    end_turn(&mut game);

    let attacker = put_in_play(&mut game, PlayerId::P0, "CHR-7K");
    put_in_play(&mut game, PlayerId::P1, "CHR-BLOCK");
    game.state.card_mut(attacker).played_on_turn = None;
    let enemy_leader = game.state.player(PlayerId::P1).leader.unwrap();

    let out = game
        .step(Action::Attack {
            attacker,
            target: enemy_leader,
        })
        .unwrap();
    let mut steps = battle_steps(&out);
    let out = game.step(Action::Block { blocker: None }).unwrap();
    steps.extend(battle_steps(&out));

    assert_eq!(
        steps,
        vec![
            BattleStep::Attack,
            BattleStep::Block,
            BattleStep::Counter,
            BattleStep::Damage,
            BattleStep::EndOfBattle,
        ]
    );
}

// ---- 8-6 Order of Effect Resolution ---------------------------------------

/// A card whose whole script is one unconditional, free auto effect.
fn auto_script(timing: Timing, ops: Vec<EffectOp>) -> CardScript {
    CardScript {
        auto: vec![AutoEffect {
            timing,
            conditions: Vec::new(),
            cost: ActivationCost::default(),
            ops,
            slot: 0,
            once_per_turn: false,
        }],
        ..CardScript::default()
    }
}

fn kos_itself() -> EffectOp {
    EffectOp::Ko {
        key: SELF_BINDING.to_string(),
    }
}

fn draw_for(player: Who) -> EffectOp {
    EffectOp::Draw { player, n: 1 }
}

/// Where in `events` the first matching event landed.
fn position_of(events: &[GameEvent], pred: impl Fn(&GameEvent) -> bool, what: &str) -> usize {
    events
        .iter()
        .position(pred)
        .unwrap_or_else(|| panic!("expected {what} in {events:#?}"))
}

fn ko_at(events: &[GameEvent]) -> usize {
    position_of(
        events,
        |e| matches!(e, GameEvent::KnockedOut { .. }),
        "a K.O.",
    )
}

fn draw_at(events: &[GameEvent], player: PlayerId) -> usize {
    position_of(
        events,
        |e| matches!(e, GameEvent::Drew { player: p, .. } if *p == player),
        &format!("a draw by {player:?}"),
    )
}

/// 8-6-3: an effect whose activation timing is fulfilled *during* another
/// effect's resolution activates after that resolution finishes. It does not
/// pre-empt the ops the triggering effect has left to run.
///
/// The shipped shape is ST08-013, whose "K.O. the battled Character, then K.O.
/// itself" fulfils ST08-001's "when a Character is K.O.'d" half-way through.
#[test]
fn rule_8_6_3_a_mid_resolution_trigger_waits_for_the_effect_that_caused_it() {
    let cards = TestCards::new();
    // K.O.s itself and then draws: the K.O. fulfils the watcher's timing with
    // an op still outstanding, which is the whole test.
    let actor = auto_script(
        Timing::EndOfYourTurn,
        vec![kos_itself(), draw_for(Who::You)],
    );
    // Draws for the *opponent*, so the two draws are told apart by player.
    let watcher = auto_script(Timing::OnCharacterKoed, vec![draw_for(Who::Opponent)]);

    let (mut game, _) = game_with(
        &cards,
        TestScripts::default()
            .with(cards.def("CHR-5K"), actor)
            .with(cards.def("CHR-7K"), watcher),
        7,
        ("LDR-001", deck_of("CHR-2K", 30)),
        ("LDR-002", deck_of("CHR-2K", 30)),
    );
    to_main(&mut game);
    put_in_play(&mut game, PlayerId::P0, "CHR-5K");
    put_in_play(&mut game, PlayerId::P0, "CHR-7K");

    let events = game.step(Action::EndMainPhase).unwrap().events;

    let ko = ko_at(&events);
    let own_draw = draw_at(&events, PlayerId::P0);
    let triggered_draw = draw_at(&events, PlayerId::P1);

    assert!(ko < own_draw, "the effect K.O.s before it draws");
    assert!(
        own_draw < triggered_draw,
        "8-6-3: the triggered effect must wait for the rest of the effect that \
         triggered it, but its draw landed at {triggered_draw} and the \
         triggering effect's own draw at {own_draw}"
    );
}

/// 8-6-1-1: where A and B are both already waiting and resolving A fulfils C's
/// timing, C resolves after B — a newly triggered effect joins the back of the
/// queue, not the front.
#[test]
fn rule_8_6_1_1_a_newly_triggered_effect_goes_behind_one_already_waiting() {
    let cards = TestCards::new();
    let first = auto_script(Timing::EndOfYourTurn, vec![kos_itself()]);
    let second = auto_script(Timing::EndOfYourTurn, vec![draw_for(Who::You)]);
    let triggered = auto_script(Timing::OnCharacterKoed, vec![draw_for(Who::Opponent)]);

    let (mut game, _) = game_with(
        &cards,
        TestScripts::default()
            .with(cards.def("CHR-5K"), first)
            .with(cards.def("CHR-7K"), second)
            .with(cards.def("CHR-2K"), triggered),
        7,
        ("LDR-001", deck_of("CHR-BLOCK", 30)),
        ("LDR-002", deck_of("CHR-BLOCK", 30)),
    );
    to_main(&mut game);
    // Both end-of-turn effects are waiting before either resolves.
    put_in_play(&mut game, PlayerId::P0, "CHR-5K");
    put_in_play(&mut game, PlayerId::P0, "CHR-7K");
    put_in_play(&mut game, PlayerId::P0, "CHR-2K");

    let events = game.step(Action::EndMainPhase).unwrap().events;

    let ko = ko_at(&events);
    let already_waiting = draw_at(&events, PlayerId::P0);
    let newly_triggered = draw_at(&events, PlayerId::P1);

    assert!(
        ko < already_waiting,
        "the first end-of-turn effect resolves before the second"
    );
    assert!(
        already_waiting < newly_triggered,
        "8-6-1-1: an effect triggered mid-resolution resolves after one that \
         was already waiting, but it cut in at {newly_triggered} ahead of \
         {already_waiting}"
    );
}

/// 8-6-1: when both players' timings are fulfilled at once, the turn player
/// resolves first.
///
/// `all_in_play` walks the turn player's board first for exactly this reason,
/// which only means anything if the queue preserves collection order.
#[test]
fn rule_8_6_1_the_turn_players_effect_resolves_before_their_opponents() {
    let cards = TestCards::new();
    let actor = auto_script(Timing::EndOfYourTurn, vec![kos_itself()]);
    // Both watchers draw for their own controller, so the order of the two
    // draws *is* the order the two effects resolved in.
    let watcher = auto_script(Timing::OnCharacterKoed, vec![draw_for(Who::You)]);

    let (mut game, _) = game_with(
        &cards,
        TestScripts::default()
            .with(cards.def("CHR-5K"), actor)
            .with(cards.def("CHR-7K"), watcher.clone())
            .with(cards.def("CHR-2K"), watcher),
        7,
        ("LDR-001", deck_of("CHR-BLOCK", 30)),
        ("LDR-002", deck_of("CHR-BLOCK", 30)),
    );
    to_main(&mut game);
    put_in_play(&mut game, PlayerId::P0, "CHR-5K");
    put_in_play(&mut game, PlayerId::P0, "CHR-7K");
    put_in_play(&mut game, PlayerId::P1, "CHR-2K");

    let events = game.step(Action::EndMainPhase).unwrap().events;

    assert!(
        draw_at(&events, PlayerId::P0) < draw_at(&events, PlayerId::P1),
        "8-6-1: the turn player's watcher must resolve before their opponent's"
    );
}

/// A card whose activated effect runs `ops`, for the vocabulary tests below.
#[cfg(test)]
fn activated_ops(ops: Vec<op_core::effect::EffectOp>) -> op_core::script::CardScript {
    op_core::script::CardScript {
        activated: vec![op_core::script::ActivatedEffect {
            conditions: vec![],
            cost: op_core::script::ActivationCost::default(),
            ops,
            slot: 0,
            once_per_turn: false,
        }],
        ..Default::default()
    }
}

/// "other than [Gecko Moria]" (ST03-004) — a negated filter excludes exactly
/// what it names and nothing else.
#[test]
fn a_negated_filter_excludes_only_what_it_names() {
    use op_core::effect::{EffectOp, Filter, Selector, Who, ALL};

    let cards = TestCards::new();
    let ko_the_others = activated_ops(vec![
        EffectOp::Choose {
            key: "k".to_string(),
            select: Selector {
                zone: Zone::Character,
                owner: Who::Opponent,
                from: None,
                up_to: ALL,
                at_least: ALL,
                filters: vec![Filter::Not(Box::new(Filter::HasName("CHR-7K".into())))],
            },
        },
        EffectOp::Ko {
            key: "k".to_string(),
        },
    ]);
    let scripts = TestScripts::default().with(cards.def("CHR-5K"), ko_the_others);
    let (mut game, _) = game_with(
        &cards,
        scripts,
        7,
        ("LDR-001", deck_of("CHR-5K", 30)),
        ("LDR-002", deck_of("CHR-5K", 30)),
    );
    to_main(&mut game);

    let source = put_in_play(&mut game, PlayerId::P0, "CHR-5K");
    let spared = put_in_play(&mut game, PlayerId::P1, "CHR-7K");
    let doomed = put_in_play(&mut game, PlayerId::P1, "CHR-5K");

    game.step(Action::ActivateEffect {
        card: source,
        slot: 0,
        discard: vec![],
    })
    .unwrap();

    assert_eq!(
        game.state.card(doomed).zone,
        Zone::Trash,
        "should be K.O.'d"
    );
    assert_eq!(
        game.state.card(spared).zone,
        Zone::Character,
        "the named card is what the filter excludes"
    );
}

/// "then shuffle your deck" (ST03-007). The shuffle is the point of the clause:
/// a search that left the deck in order would tell the player their next draw.
#[test]
fn a_shuffle_reorders_the_deck() {
    use op_core::effect::{EffectOp, Who};

    let cards = TestCards::new();
    let scripts = TestScripts::default().with(
        cards.def("CHR-5K"),
        activated_ops(vec![EffectOp::Shuffle { player: Who::You }]),
    );
    let (mut game, _) = game_with(
        &cards,
        scripts,
        7,
        ("LDR-001", deck_of("CHR-5K", 30)),
        ("LDR-002", deck_of("CHR-5K", 30)),
    );
    to_main(&mut game);

    let source = put_in_play(&mut game, PlayerId::P0, "CHR-5K");
    let before = game.state.player(PlayerId::P0).deck.clone();
    let opponent_before = game.state.player(PlayerId::P1).deck.clone();

    game.step(Action::ActivateEffect {
        card: source,
        slot: 0,
        discard: vec![],
    })
    .unwrap();

    let after = game.state.player(PlayerId::P0).deck.clone();
    assert_ne!(before, after, "the deck should have been reordered");
    let mut sorted_before = before.clone();
    let mut sorted_after = after.clone();
    sorted_before.sort();
    sorted_after.sort();
    assert_eq!(sorted_before, sorted_after, "same cards, different order");
    assert_eq!(
        game.state.player(PlayerId::P1).deck,
        opponent_before,
        "\"your deck\" is the controller's alone"
    );
}

/// "draw 1 card if you have 3 or less cards in your hand" (ST03-017) — the
/// condition reads the controller's hand at the moment it is evaluated.
#[test]
fn a_hand_size_condition_gates_on_the_controllers_hand() {
    use op_core::effect::{Condition, EffectOp, Who};

    let cards = TestCards::new();
    let scripts = TestScripts::default().with(
        cards.def("CHR-5K"),
        activated_ops(vec![
            EffectOp::RequireIf {
                cond: Condition::HandAtMost(3),
            },
            EffectOp::Draw {
                player: Who::You,
                n: 1,
            },
        ]),
    );
    let (mut game, _) = game_with(
        &cards,
        scripts,
        7,
        ("LDR-001", deck_of("CHR-5K", 30)),
        ("LDR-002", deck_of("CHR-5K", 30)),
    );
    to_main(&mut game);
    let source = put_in_play(&mut game, PlayerId::P0, "CHR-5K");

    // An opening hand is 5, so the condition fails and the draw is skipped.
    assert!(game.state.player(PlayerId::P0).hand.len() > 3);
    let deck_before = game.state.player(PlayerId::P0).deck.len();
    game.step(Action::ActivateEffect {
        card: source,
        slot: 0,
        discard: vec![],
    })
    .unwrap();
    assert_eq!(
        game.state.player(PlayerId::P0).deck.len(),
        deck_before,
        "a hand over the threshold should not draw"
    );

    // Trim to exactly the threshold and it fires — the boundary is "or less".
    while game.state.player(PlayerId::P0).hand.len() > 3 {
        let card = game.state.player(PlayerId::P0).hand[0];
        game.state
            .move_card(card, PlayerId::P0, Zone::Trash, Placement::Bottom);
    }
    let deck_before = game.state.player(PlayerId::P0).deck.len();
    game.step(Action::ActivateEffect {
        card: source,
        slot: 0,
        discard: vec![],
    })
    .unwrap();
    assert_eq!(
        game.state.player(PlayerId::P0).deck.len(),
        deck_before - 1,
        "a hand of exactly 3 is \"3 or less\""
    );
}

/// "Look at 3 cards from the top of your deck and return them to the top or
/// bottom of the deck in any order" (ST03-010), taken literally: the order is
/// the player's, both ends of it.
#[test]
fn a_look_top_arrangement_is_placed_exactly_as_asked() {
    use op_core::effect::EffectOp;

    let cards = TestCards::new();
    // Activated rather than [On Play]: `put_in_play` moves the card straight
    // into the Character area, so no play-time timing fires from it.
    let look = op_core::script::CardScript {
        activated: vec![op_core::script::ActivatedEffect {
            conditions: vec![],
            cost: op_core::script::ActivationCost::default(),
            ops: vec![EffectOp::LookTop {
                n: 3,
                key: "l".to_string(),
            }],
            slot: 0,
            once_per_turn: false,
        }],
        ..Default::default()
    };
    let scripts = TestScripts::default().with(cards.def("CHR-5K"), look);
    let (mut game, _) = game_with(
        &cards,
        scripts,
        7,
        ("LDR-001", deck_of("CHR-5K", 30)),
        ("LDR-002", deck_of("CHR-7K", 30)),
    );
    to_main(&mut game);

    let deck_before = game.state.player(PlayerId::P0).deck.clone();
    let looked: Vec<_> = deck_before[..3].to_vec();
    let rest: Vec<_> = deck_before[3..].to_vec();

    let source = put_in_play(&mut game, PlayerId::P0, "CHR-5K");
    game.step(Action::ActivateEffect {
        card: source,
        slot: 0,
        discard: vec![],
    })
    .unwrap();

    let pending = game.pending().cloned();
    let Some(Pending::Arrange { cards: offered, .. }) = pending else {
        panic!("expected an Arrange decision, got {pending:?}");
    };
    assert_eq!(offered, looked, "the top three, top-first");

    // Deliberately not the order they came off in: the middle card is buried,
    // and the other two come back in swapped order. A placement that ignored
    // the answer would leave the deck as it was and pass a weaker assertion.
    let top = vec![looked[2], looked[0]];
    let bottom = vec![looked[1]];
    game.step(Action::Arrange {
        top: top.clone(),
        bottom: bottom.clone(),
    })
    .unwrap();

    let deck_after = game.state.player(PlayerId::P0).deck.clone();
    let mut expected = top;
    expected.extend(rest);
    expected.extend(bottom);
    assert_eq!(deck_after, expected, "deck read top-to-bottom");
}

/// An arrangement that leaves a card out is refused rather than tolerated —
/// accepting it would strand the missing card in limbo, in no area at all.
#[test]
fn an_arrangement_that_drops_a_card_is_illegal() {
    use op_core::effect::EffectOp;

    let cards = TestCards::new();
    // Activated rather than [On Play]: `put_in_play` moves the card straight
    // into the Character area, so no play-time timing fires from it.
    let look = op_core::script::CardScript {
        activated: vec![op_core::script::ActivatedEffect {
            conditions: vec![],
            cost: op_core::script::ActivationCost::default(),
            ops: vec![EffectOp::LookTop {
                n: 3,
                key: "l".to_string(),
            }],
            slot: 0,
            once_per_turn: false,
        }],
        ..Default::default()
    };
    let scripts = TestScripts::default().with(cards.def("CHR-5K"), look);
    let (mut game, _) = game_with(
        &cards,
        scripts,
        7,
        ("LDR-001", deck_of("CHR-5K", 30)),
        ("LDR-002", deck_of("CHR-7K", 30)),
    );
    to_main(&mut game);
    let source = put_in_play(&mut game, PlayerId::P0, "CHR-5K");
    game.step(Action::ActivateEffect {
        card: source,
        slot: 0,
        discard: vec![],
    })
    .unwrap();

    let Some(Pending::Arrange { cards: offered, .. }) = game.pending().cloned() else {
        panic!("expected an Arrange decision");
    };

    let out = game.step(Action::Arrange {
        top: vec![offered[0]],
        bottom: vec![offered[1]],
    });
    assert!(out.is_err(), "two of three cards placed should be refused");
    assert!(
        matches!(game.pending(), Some(Pending::Arrange { .. })),
        "the decision should still be pending"
    );
}

/// A [Blocker] carrying one [On Block] effect built from `ops`.
fn on_block(ops: Vec<op_core::effect::EffectOp>) -> op_core::script::CardScript {
    op_core::script::CardScript {
        auto: vec![op_core::script::AutoEffect {
            timing: op_core::effect::Timing::OnBlock,
            conditions: vec![],
            cost: op_core::script::ActivationCost::default(),
            ops,
            slot: 0,
            once_per_turn: false,
        }],
        ..Default::default()
    }
}

/// A [Blocker] whose only text is an [On Block] draw.
fn draws_on_block() -> op_core::script::CardScript {
    on_block(vec![op_core::effect::EffectOp::Draw {
        player: op_core::effect::Who::You,
        n: 1,
    }])
}

/// A board where P0's 7000-power attacker faces P1's [Blocker], on a turn where
/// battles are legal. Returns the game, the attacker and P1's Leader — the
/// blocker itself comes back from `legal_blockers` once an attack is declared.
fn on_block_fixture(
    script: op_core::script::CardScript,
) -> (Game, op_core::CardInstanceId, op_core::CardInstanceId) {
    let cards = TestCards::new();
    let scripts = TestScripts::default().with(cards.def("CHR-BLOCK"), script);
    let (mut game, _) = game_with(
        &cards,
        scripts,
        7,
        ("LDR-001", deck_of("CHR-7K", 30)),
        ("LDR-002", deck_of("CHR-BLOCK", 30)),
    );
    to_main(&mut game);
    end_turn(&mut game);
    end_turn(&mut game);

    let attacker = put_in_play(&mut game, PlayerId::P0, "CHR-7K");
    put_in_play(&mut game, PlayerId::P1, "CHR-BLOCK");
    // Clear summoning sickness so the attacker may declare.
    game.state.card_mut(attacker).played_on_turn = None;
    let enemy_leader = game.state.player(PlayerId::P1).leader.unwrap();
    (game, attacker, enemy_leader)
}

/// 7-1-2-2 and 10-2-15-1: activating a [Blocker] is what fulfils [On Block].
#[test]
fn rule_7_1_2_2_activating_a_blocker_fires_on_block() {
    let (mut game, attacker, enemy_leader) = on_block_fixture(draws_on_block());

    // Deck size, not hand size: taking damage also puts a Life card in hand
    // (10-1-5-2), so hand size cannot tell a draw from a life card. Only a
    // draw comes off the deck.
    let before = game.state.player(PlayerId::P1).deck.len();
    game.step(Action::Attack {
        attacker,
        target: enemy_leader,
    })
    .unwrap();
    let blocker = game.legal_blockers()[0];
    game.step(Action::Block {
        blocker: Some(blocker),
    })
    .unwrap();

    assert_eq!(
        game.state.player(PlayerId::P1).deck.len(),
        before - 1,
        "[On Block] should have drawn for the blocking player"
    );
}

/// The other half of the same wiring, and the reason the test above is not
/// enough on its own: the Block Step is entered whether or not anyone blocks,
/// so firing on the step rather than the activation would pass that one and
/// fail this.
#[test]
fn rule_7_1_2_2_declining_to_block_fires_nothing() {
    let (mut game, attacker, enemy_leader) = on_block_fixture(draws_on_block());

    // Deck size again, and here it is load-bearing rather than tidy: letting a
    // 7000-power attacker through costs a Life card, which lands in hand and
    // would read exactly like the draw this test says must not happen.
    let before = game.state.player(PlayerId::P1).deck.len();
    game.step(Action::Attack {
        attacker,
        target: enemy_leader,
    })
    .unwrap();
    game.step(Action::Block { blocker: None }).unwrap();

    assert_eq!(
        game.state.player(PlayerId::P1).deck.len(),
        before,
        "no [Blocker] was activated, so nothing should have fired"
    );
}

/// 7-1-2-3: if an [On Block] effect moves the attacker out of its area, the
/// battle skips the Counter Step and ends. ST03-003 is the printed case — it
/// places a Character at the bottom of the deck, which can be the attacker.
#[test]
fn rule_7_1_2_3_an_on_block_effect_removing_the_attacker_ends_the_battle() {
    use op_core::effect::{EffectOp, Selector, Who, ALL};

    let bounce_the_attacker = on_block(vec![
        EffectOp::Choose {
            key: "a".to_string(),
            select: Selector {
                zone: Zone::Character,
                owner: Who::Opponent,
                from: None,
                up_to: ALL,
                at_least: ALL,
                filters: vec![],
            },
        },
        EffectOp::MoveTo {
            key: "a".to_string(),
            to: Zone::Hand,
        },
    ]);
    let (mut game, attacker, enemy_leader) = on_block_fixture(bounce_the_attacker);

    game.step(Action::Attack {
        attacker,
        target: enemy_leader,
    })
    .unwrap();
    let blocker = game.legal_blockers()[0];
    let out = game
        .step(Action::Block {
            blocker: Some(blocker),
        })
        .unwrap();

    assert_eq!(game.state.card(attacker).zone, Zone::Hand);
    assert!(game.state.battle.is_none(), "the battle should have ended");
    // The Counter Step, not just the Damage Step: 7-1-2-3 says the battle
    // proceeds to the end of the battle rather than to the Counter Step, so a
    // Counter Step announced to clients and to the log is the rule being
    // broken even though nothing further happens in it.
    let steps = battle_steps(&out);
    assert!(
        !steps.contains(&BattleStep::Counter) && !steps.contains(&BattleStep::Damage),
        "7-1-2-3 proceeds straight to the end of the battle, got {steps:?}"
    );
}
