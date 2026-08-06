//! Rules conformance suite.
//!
//! Tests are named for the Comprehensive Rules v1.2.0 clause they pin down, so
//! that a failure points straight at the rule text it violates.

mod common;

use common::{deck_of, game_with, TestCards, TestScripts};
use op_core::state::Placement;
use op_core::zone::Zone;
use op_core::{Action, BattleStep, GameEvent, Game, GameOver, Pending, PlayerId};

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

#[test]
fn rule_6_5_6_1_no_battles_on_the_first_turn() {
    let (_cards, mut game) = fixture();
    to_main(&mut game);
    let leader = game.state.player(PlayerId::P0).leader.unwrap();
    let target = game.state.player(PlayerId::P1).leader.unwrap();
    assert!(game
        .step(Action::Attack {
            attacker: leader,
            target
        })
        .is_err());
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
    assert_eq!(game.state.player(PlayerId::P1).trash.len(), trash_before + 1);
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

#[test]
fn rule_3_7_6_character_area_holds_at_most_five() {
    let cards = TestCards::new();
    let (mut game, _) = game_with(
        &cards,
        TestScripts::default(),
        7,
        ("LDR-001", deck_of("CHR-2K", 30)),
        ("LDR-002", deck_of("CHR-2K", 30)),
    );
    to_main(&mut game);
    for _ in 0..5 {
        put_in_play(&mut game, PlayerId::P0, "CHR-2K");
    }
    assert_eq!(game.state.player(PlayerId::P0).characters.len(), 5);

    // With a full area, playing a sixth is rejected rather than silently
    // overflowing (3-7-6-1 requires a trash-to-make-room choice, not yet
    // supported).
    let in_hand = game.state.player(PlayerId::P0).hand[0];
    assert!(game.step(Action::PlayCard { card: in_hand }).is_err());
}

// ---- battle step sequencing ------------------------------------------------

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

    let mut steps = Vec::new();
    let out = game
        .step(Action::Attack {
            attacker,
            target: enemy_leader,
        })
        .unwrap();
    steps.extend(out.events.iter().filter_map(|e| match e {
        GameEvent::BattleStepStarted { step } => Some(*step),
        _ => None,
    }));
    let out = game.step(Action::Block { blocker: None }).unwrap();
    steps.extend(out.events.iter().filter_map(|e| match e {
        GameEvent::BattleStepStarted { step } => Some(*step),
        _ => None,
    }));

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
