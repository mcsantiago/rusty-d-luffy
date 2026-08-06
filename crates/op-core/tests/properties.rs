//! Property tests for the guarantees the architecture depends on:
//! determinism, hidden-information safety, and termination.

mod common;

use common::{deck_of, game_with, TestCards, TestScripts};
use op_core::view::PlayerView;
use op_core::zone::Zone;
use op_core::{legal_actions, Action, Game, GameEvent, PlayerId};

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

/// Plays a whole game by picking uniformly among legal actions, returning the
/// action sequence taken and the state hash after each one.
fn random_playout(seed: u64, policy_seed: u64) -> (Vec<Action>, Vec<u64>) {
    let cards = TestCards::new();
    let (mut game, _) = game_with(
        &cards,
        TestScripts::default(),
        seed,
        ("LDR-001", deck_of("CHR-5K", 40)),
        ("LDR-002", deck_of("CHR-BLOCK", 40)),
    );

    let mut policy = StdRng::seed_from_u64(policy_seed);
    let mut actions = Vec::new();
    let mut hashes = Vec::new();

    for _ in 0..4000 {
        if game.is_over() || game.pending().is_none() {
            break;
        }
        let legal = legal_actions(&game);
        assert!(
            !legal.is_empty(),
            "no legal action available at pending {:?}",
            game.pending()
        );
        let action = legal[policy.gen_range(0..legal.len())].clone();
        game.step(action.clone())
            .unwrap_or_else(|e| panic!("legal action {action:?} was rejected: {e}"));
        actions.push(action);
        hashes.push(game.state.state_hash());
    }

    (actions, hashes)
}

#[test]
fn identical_seeds_and_actions_produce_identical_states() {
    for seed in 0..8u64 {
        let (a_actions, a_hashes) = random_playout(seed, seed + 100);
        let (b_actions, b_hashes) = random_playout(seed, seed + 100);
        assert_eq!(a_actions, b_actions, "action sequences diverged (seed {seed})");
        assert_eq!(a_hashes, b_hashes, "state hashes diverged (seed {seed})");
    }
}

#[test]
fn replaying_a_recorded_action_list_reproduces_the_game() {
    let seed = 42;
    let (actions, hashes) = random_playout(seed, 7);

    // Replay the recorded actions into a fresh game built from the same seed.
    let cards = TestCards::new();
    let (mut game, _) = game_with(
        &cards,
        TestScripts::default(),
        seed,
        ("LDR-001", deck_of("CHR-5K", 40)),
        ("LDR-002", deck_of("CHR-BLOCK", 40)),
    );

    for (i, action) in actions.iter().enumerate() {
        game.step(action.clone())
            .unwrap_or_else(|e| panic!("replay diverged at action {i}: {e}"));
        assert_eq!(
            game.state.state_hash(),
            hashes[i],
            "replay diverged at action {i}"
        );
    }
}

#[test]
fn serializing_and_restoring_mid_game_preserves_the_position() {
    let cards = TestCards::new();
    let (mut game, _) = game_with(
        &cards,
        TestScripts::default(),
        11,
        ("LDR-001", deck_of("CHR-5K", 40)),
        ("LDR-002", deck_of("CHR-BLOCK", 40)),
    );

    let mut policy = StdRng::seed_from_u64(3);
    for _ in 0..60 {
        if game.is_over() {
            break;
        }
        let legal = legal_actions(&game);
        if legal.is_empty() {
            break;
        }
        game.step(legal[policy.gen_range(0..legal.len())].clone())
            .unwrap();
    }

    let json = serde_json::to_string(&game.state).expect("state must serialize");
    let restored: op_core::GameState =
        serde_json::from_str(&json).expect("state must round-trip");

    assert_eq!(restored.state_hash(), game.state.state_hash());
    assert_eq!(restored, game.state);
}

/// The wire format must never name a card the viewer could not legitimately
/// identify.
///
/// This is stricter than "don't send the card number": `CardInstanceId`s are
/// assigned in decklist order at setup, so an id is close to a direct function
/// of the card number for anyone holding the decklist. An id is only safe to
/// send for a card in an open area, or in the viewer's own hand.
#[test]
fn the_event_stream_never_names_a_card_the_viewer_cannot_identify() {
    let cards = TestCards::new();
    let (mut game, opening) = game_with(
        &cards,
        TestScripts::default(),
        23,
        ("LDR-001", deck_of("CHR-5K", 40)),
        ("LDR-002", deck_of("CHR-BLOCK", 40)),
    );

    let mut policy = StdRng::seed_from_u64(5);
    let mut outcome = opening;
    let mut checked = 0;

    for _ in 0..500 {
        for viewer in [PlayerId::P0, PlayerId::P1] {
            let projected = outcome.for_player(&game.state, viewer);

            for event in &projected.events {
                for id in event.exposed_ids() {
                    let card = game.state.card(id);
                    let legitimate =
                        card.zone.is_open() || (card.zone == Zone::Hand && card.controller == viewer);
                    assert!(
                        legitimate,
                        "{viewer:?} was told about card {id:?} in {:?} (controller {:?}) \
                         via {event:?}",
                        card.zone, card.controller
                    );
                    checked += 1;
                }
            }

            // A decision the opponent is facing is itself informative.
            if let Some(pending) = &projected.pending {
                assert_eq!(pending.player(), viewer);
            }
        }

        if game.is_over() {
            break;
        }
        let legal = legal_actions(&game);
        if legal.is_empty() {
            break;
        }
        outcome = game
            .step(legal[policy.gen_range(0..legal.len())].clone())
            .unwrap();
    }

    assert!(checked > 100, "test exercised too few events ({checked})");
}

/// A card drawn by the opponent must never be identifiable, and the viewer's
/// own draw must be.
#[test]
fn draws_are_visible_only_to_the_drawing_player() {
    let cards = TestCards::new();
    let (mut game, _) = game_with(
        &cards,
        TestScripts::default(),
        3,
        ("LDR-001", deck_of("CHR-5K", 40)),
        ("LDR-002", deck_of("CHR-BLOCK", 40)),
    );

    let mut policy = StdRng::seed_from_u64(8);
    let mut seen_own = false;
    let mut seen_opponent = false;

    for _ in 0..300 {
        if game.is_over() {
            break;
        }
        let legal = legal_actions(&game);
        if legal.is_empty() {
            break;
        }
        let outcome = game
            .step(legal[policy.gen_range(0..legal.len())].clone())
            .unwrap();

        for event in &outcome.events {
            let GameEvent::Drew { player, .. } = event else {
                continue;
            };
            let drawer = *player;
            let watcher = drawer.opponent();

            match event.project(&game.state, drawer) {
                op_core::PlayerEvent::Drew { card, .. } => {
                    assert!(!card.is_hidden(), "a player must see their own draw");
                    seen_own = true;
                }
                other => panic!("projection changed the variant: {other:?}"),
            }
            match event.project(&game.state, watcher) {
                op_core::PlayerEvent::Drew { card, .. } => {
                    assert!(card.is_hidden(), "an opponent's draw must be hidden");
                    seen_opponent = true;
                }
                other => panic!("projection changed the variant: {other:?}"),
            }
        }
    }

    assert!(seen_own && seen_opponent, "no draws were exercised");
}

#[test]
fn player_views_never_leak_hidden_information() {
    let cards = TestCards::new();
    let (mut game, _) = game_with(
        &cards,
        TestScripts::default(),
        5,
        ("LDR-001", deck_of("CHR-5K", 40)),
        ("LDR-002", deck_of("CHR-BLOCK", 40)),
    );

    let mut policy = StdRng::seed_from_u64(9);
    for _ in 0..400 {
        if game.is_over() {
            break;
        }
        let legal = legal_actions(&game);
        if legal.is_empty() {
            break;
        }

        let derived = game.derived();
        for viewer in [PlayerId::P0, PlayerId::P1] {
            let view = PlayerView::project(&game.state, game.db(), &derived, viewer);
            let opp = viewer.opponent();

            // 3-4-2: the opponent's hand is secret.
            for card in &view.opponent.hand {
                assert!(
                    card.number.is_none(),
                    "opponent hand leaked a card identity"
                );
            }
            // 3-1-4: counts are public, contents are not.
            assert_eq!(view.opponent.hand_count, game.state.player(opp).hand.len());
            assert_eq!(view.opponent.deck_count, game.state.player(opp).deck.len());
            assert_eq!(view.opponent.life_count, game.state.player(opp).life.len());
            // A pending decision is only shown to the player who must make it.
            if let Some(pending) = &view.pending {
                assert_eq!(pending.player(), viewer);
            }
        }

        game.step(legal[policy.gen_range(0..legal.len())].clone())
            .unwrap();
    }
}

#[test]
fn random_games_terminate_without_panicking_or_stalling() {
    let mut finished = 0;
    for seed in 0..64u64 {
        let cards = TestCards::new();
        let (mut game, _) = game_with(
            &cards,
            TestScripts::default(),
            seed,
            ("LDR-001", deck_of("CHR-5K", 40)),
            ("LDR-002", deck_of("CHR-BLOCK", 40)),
        );

        let mut policy = StdRng::seed_from_u64(seed * 31 + 1);
        let mut steps = 0;
        while !game.is_over() {
            let legal = legal_actions(&game);
            assert!(!legal.is_empty(), "stalled at {:?}", game.pending());
            game.step(legal[policy.gen_range(0..legal.len())].clone())
                .unwrap();
            steps += 1;
            assert!(steps < 5000, "game {seed} did not terminate in 5000 actions");
        }
        assert!(game.result().is_some());
        finished += 1;
    }
    assert_eq!(finished, 64);
}

/// Every action the generator offers must be accepted, and every action it
/// omits must be rejected. This is what lets the server trust the generator as
/// its validator.
#[test]
fn legal_action_generator_agrees_with_the_engine() {
    let cards = TestCards::new();
    let (mut game, _) = game_with(
        &cards,
        TestScripts::default(),
        17,
        ("LDR-001", deck_of("CHR-5K", 40)),
        ("LDR-002", deck_of("CHR-BLOCK", 40)),
    );

    let mut policy = StdRng::seed_from_u64(4);
    for _ in 0..300 {
        if game.is_over() {
            break;
        }
        let legal = legal_actions(&game);
        if legal.is_empty() {
            break;
        }

        // Each offered action must be accepted by a clone of the engine.
        for action in &legal {
            let mut probe: Game = game.clone();
            probe
                .step(action.clone())
                .unwrap_or_else(|e| panic!("generator offered rejected action {action:?}: {e}"));
        }

        game.step(legal[policy.gen_range(0..legal.len())].clone())
            .unwrap();
    }
}
