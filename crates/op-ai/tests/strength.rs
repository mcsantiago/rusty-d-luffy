//! Agent strength and safety.
//!
//! The point of these tests is not that a particular win rate is correct, but
//! that the ordering holds: search beats one-ply, one-ply beats random. If that
//! inverts, either the evaluation or the search is broken.

use std::sync::Arc;

use op_ai::{play_out, Agent, HeuristicAgent, IsmctsAgent, IsmctsConfig};
use op_cards::Cards;
use op_core::card::CardDb;
use op_core::{legal_actions, Action, DeckList, Game, GameConfig, PlayerId};

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

/// Uniform random over legal actions — the floor any real agent must clear.
struct RandomAgent(StdRng);

impl Agent for RandomAgent {
    fn choose(&mut self, game: &Game, _player: PlayerId) -> Action {
        let legal = legal_actions(game);
        legal[self.0.gen_range(0..legal.len())].clone()
    }
}

fn deck(leader: &str, spec: &[(&str, usize)]) -> DeckList {
    let mut cards = Vec::new();
    for (number, n) in spec {
        for _ in 0..*n {
            cards.push(number.to_string());
        }
    }
    DeckList {
        leader: leader.into(),
        cards,
    }
}

fn st01() -> DeckList {
    deck(
        "ST01-001",
        &[
            ("ST01-002", 4),
            ("ST01-003", 4),
            ("ST01-004", 4),
            ("ST01-005", 2),
            ("ST01-006", 4),
            ("ST01-007", 4),
            ("ST01-008", 2),
            ("ST01-009", 4),
            ("ST01-010", 2),
            ("ST01-011", 4),
            ("ST01-012", 2),
            ("ST01-013", 4),
            ("ST01-014", 4),
            ("ST01-015", 2),
            ("ST01-016", 2),
            ("ST01-017", 2),
        ],
    )
}

fn st02() -> DeckList {
    deck(
        "ST02-001",
        &[
            ("ST02-002", 4),
            ("ST02-003", 4),
            ("ST02-004", 4),
            ("ST02-005", 4),
            ("ST02-006", 2),
            ("ST02-007", 4),
            ("ST02-008", 4),
            ("ST02-009", 2),
            ("ST02-010", 2),
            ("ST02-011", 4),
            ("ST02-012", 4),
            ("ST02-013", 2),
            ("ST02-014", 2),
            ("ST02-015", 4),
            ("ST02-016", 2),
            ("ST02-017", 2),
        ],
    )
}

type Scripts = Arc<dyn op_core::script::ScriptSource + Send + Sync>;

fn load() -> Option<(Arc<CardDb>, Scripts)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/cards");
    let db = CardDb::load_dir(dir).ok()?;
    let cards: Scripts = Arc::new(Cards::new(&db));
    Some((Arc::new(db), cards))
}

/// Runs `games` matches, alternating who goes first so seat advantage cancels.
/// Returns wins for the agent built by `challenger`.
fn match_up(
    games: u32,
    mut challenger: impl FnMut(u64) -> Box<dyn Agent>,
    mut baseline: impl FnMut(u64) -> Box<dyn Agent>,
) -> Option<(u32, u32)> {
    let (db, cards) = load()?;
    let (mut wins, mut losses) = (0, 0);

    for seed in 0..games as u64 {
        // Alternate seats: on odd seeds the challenger goes second.
        let challenger_seat = if seed % 2 == 0 {
            PlayerId::P0
        } else {
            PlayerId::P1
        };

        let config = GameConfig {
            seed,
            first_player: PlayerId::P0,
            decks: [st01(), st02()],
            allow_illegal_decks: false,
        };
        let (mut game, _) =
            Game::new(config, Arc::clone(&db), Arc::clone(&cards)).expect("legal decks");

        let mut a = challenger(seed);
        let mut b = baseline(seed);
        let result = if challenger_seat == PlayerId::P0 {
            play_out(&mut game, a.as_mut(), b.as_mut(), 20_000)
        } else {
            play_out(&mut game, b.as_mut(), a.as_mut(), 20_000)
        };

        match result.and_then(|r| r.winner()) {
            Some(w) if w == challenger_seat => wins += 1,
            Some(_) => losses += 1,
            None => {}
        }
    }
    Some((wins, losses))
}

#[test]
fn heuristic_agent_beats_random() {
    let Some((wins, losses)) = match_up(
        40,
        |seed| Box::new(HeuristicAgent::new(StdRng::seed_from_u64(seed))),
        |seed| Box::new(RandomAgent(StdRng::seed_from_u64(seed ^ 0xABCD))),
    ) else {
        eprintln!("skipping: run tools/ingest/fetch_cards.py");
        return;
    };

    println!("heuristic vs random: {wins}-{losses}");
    assert!(
        wins > losses * 2,
        "heuristic should dominate random, got {wins}-{losses}"
    );
}

#[test]
fn ismcts_beats_the_heuristic_it_rolls_out_with() {
    let Some((wins, losses)) = match_up(
        20,
        |seed| {
            Box::new(IsmctsAgent::new(IsmctsConfig {
                iterations: 120,
                rollout_depth: 40,
                seed,
                ..Default::default()
            }))
        },
        |seed| Box::new(HeuristicAgent::new(StdRng::seed_from_u64(seed ^ 0x1234))),
    ) else {
        eprintln!("skipping: run tools/ingest/fetch_cards.py");
        return;
    };

    println!("ismcts vs heuristic: {wins}-{losses}");
    // Search should be ahead of its own rollout policy. The margin is loose
    // because 20 games at 120 iterations is a noisy measurement.
    assert!(
        wins >= losses,
        "search should not lose to its rollout policy, got {wins}-{losses}"
    );
}

#[test]
fn agents_never_produce_an_illegal_action() {
    let Some((db, cards)) = load() else { return };
    for seed in 0..6u64 {
        let config = GameConfig {
            seed,
            first_player: PlayerId::P0,
            decks: [st01(), st02()],
            allow_illegal_decks: false,
        };
        let (mut game, _) =
            Game::new(config, Arc::clone(&db), Arc::clone(&cards)).expect("legal decks");

        let mut p0 = IsmctsAgent::new(IsmctsConfig {
            iterations: 40,
            rollout_depth: 25,
            seed,
            ..Default::default()
        });
        let mut p1 = HeuristicAgent::new(StdRng::seed_from_u64(seed));

        let mut actions = 0;
        while !game.is_over() {
            let Some(pending) = game.pending() else { break };
            let actor = pending.player();
            let action = if actor == PlayerId::P0 {
                p0.choose(&game, actor)
            } else {
                p1.choose(&game, actor)
            };
            game.step(action.clone())
                .unwrap_or_else(|e| panic!("agent produced illegal action {action:?}: {e}"));
            actions += 1;
            assert!(actions < 20_000, "game did not terminate");
        }
    }
}

#[test]
fn determinization_preserves_everything_the_observer_can_see() {
    let Some((db, cards)) = load() else { return };
    let config = GameConfig {
        seed: 9,
        first_player: PlayerId::P0,
        decks: [st01(), st02()],
        allow_illegal_decks: false,
    };
    let (mut game, _) = Game::new(config, db, cards).expect("legal decks");

    // Advance a few turns so there is real hidden state to scramble.
    let mut policy = StdRng::seed_from_u64(2);
    for _ in 0..80 {
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

    let before = game.state.clone();
    let mut after = before.clone();
    op_ai::determinize::determinize(&mut after, PlayerId::P0, &mut policy);

    for player in [PlayerId::P0, PlayerId::P1] {
        for zone in [
            op_core::Zone::Deck,
            op_core::Zone::Hand,
            op_core::Zone::Life,
            op_core::Zone::Character,
            op_core::Zone::Trash,
            op_core::Zone::Cost,
        ] {
            assert_eq!(
                before.player(player).zone(zone).len(),
                after.player(player).zone(zone).len(),
                "{player:?} {zone:?} changed size"
            );
        }
    }

    // The observer's own hand is known to them and must be untouched.
    let hand_defs = |s: &op_core::GameState| -> Vec<_> {
        s.player(PlayerId::P0)
            .hand
            .iter()
            .map(|&id| s.card(id).def)
            .collect()
    };
    assert_eq!(hand_defs(&before), hand_defs(&after));

    // Open areas must be untouched.
    let board_defs = |s: &op_core::GameState| -> Vec<_> {
        s.all_in_play().iter().map(|&id| s.card(id).def).collect()
    };
    assert_eq!(board_defs(&before), board_defs(&after));
}
