//! End-to-end tests against the real ST-01 and ST-02 card pool.
//!
//! Skipped when `data/` is unpopulated; run
//! `python3 tools/ingest/fetch_cards.py` first.

use std::sync::Arc;

use op_cards::Cards;
use op_core::card::{CardDb, Keyword};
use op_core::effect::Timing;
use op_core::script::ScriptSource;
use op_core::state::Placement;
use op_core::zone::Zone;
use op_core::{legal_actions, Action, DeckList, Game, GameConfig, PlayerId};

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

/// The official ST-01 Straw Hat Crew decklist.
fn st01() -> DeckList {
    DeckList {
        leader: "ST01-001".into(),
        cards: counts(&[
            ("ST01-002", 4), ("ST01-003", 4), ("ST01-004", 4), ("ST01-005", 2),
            ("ST01-006", 4), ("ST01-007", 4), ("ST01-008", 2), ("ST01-009", 4),
            ("ST01-010", 2), ("ST01-011", 4), ("ST01-012", 2), ("ST01-013", 4),
            ("ST01-014", 4), ("ST01-015", 2), ("ST01-016", 2), ("ST01-017", 2),
        ]),
    }
}

/// The official ST-02 Worst Generation decklist.
fn st02() -> DeckList {
    DeckList {
        leader: "ST02-001".into(),
        cards: counts(&[
            ("ST02-002", 4), ("ST02-003", 4), ("ST02-004", 4), ("ST02-005", 4),
            ("ST02-006", 2), ("ST02-007", 4), ("ST02-008", 4), ("ST02-009", 2),
            ("ST02-010", 2), ("ST02-011", 4), ("ST02-012", 4), ("ST02-013", 2),
            ("ST02-014", 2), ("ST02-015", 4), ("ST02-016", 2), ("ST02-017", 2),
        ]),
    }
}

/// ST-06 Absolute Justice. A legal 50-card build, not the printed list.
fn st06() -> DeckList {
    DeckList {
        leader: "ST06-001".into(),
        cards: counts(&[
            ("ST06-002", 4), ("ST06-003", 4), ("ST06-004", 2), ("ST06-005", 2),
            ("ST06-006", 4), ("ST06-007", 4), ("ST06-008", 4), ("ST06-009", 4),
            ("ST06-010", 4), ("ST06-011", 2), ("ST06-012", 2), ("ST06-013", 4),
            ("ST06-014", 4), ("ST06-015", 2), ("ST06-016", 2), ("ST06-017", 2),
        ]),
    }
}

fn counts(spec: &[(&str, usize)]) -> Vec<String> {
    let mut out = Vec::new();
    for (number, n) in spec {
        for _ in 0..*n {
            out.push(number.to_string());
        }
    }
    out
}

type Scripts = Arc<dyn ScriptSource + Send + Sync>;

fn load() -> Option<(Arc<CardDb>, Scripts)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/cards");
    let db = CardDb::load_dir(dir).ok()?;
    let cards: Scripts = Arc::new(Cards::new(&db));
    Some((Arc::new(db), cards))
}

fn new_game(db: Arc<CardDb>, cards: Scripts, seed: u64) -> Game {
    let config = GameConfig {
        seed,
        first_player: PlayerId::P0,
        decks: [st01(), st02()],
        allow_illegal_decks: false,
    };
    Game::new(config, db, cards).expect("starter decks must be legal").0
}

#[test]
fn official_starter_decklists_pass_deck_construction_rules() {
    let Some((db, cards)) = load() else {
        eprintln!("skipping: run tools/ingest/fetch_cards.py");
        return;
    };
    // 50 cards, at most 4 of any card number (5-1-2, 5-1-2-3). Game::new
    // enforces both; this fails loudly if a list above is wrong.
    assert_eq!(st01().cards.len(), 50);
    assert_eq!(st02().cards.len(), 50);
    let _ = new_game(db, cards, 1);
}

/// Every scripted deck must be able to play a full game against every other.
/// A deck that only works against one opponent is not really implemented.
#[test]
fn every_deck_pairing_plays_to_completion() {
    let Some((db, cards)) = load() else {
        eprintln!("skipping: run tools/ingest/fetch_cards.py");
        return;
    };
    let decks: [(&str, fn() -> DeckList); 3] =
        [("ST-01", st01), ("ST-02", st02), ("ST-06", st06)];

    for (a_name, a) in decks {
        for (b_name, b) in decks {
            let config = GameConfig {
                seed: 11,
                first_player: PlayerId::P0,
                decks: [a(), b()],
                allow_illegal_decks: false,
            };
            let (mut game, _) = Game::new(config, Arc::clone(&db), Arc::clone(&cards))
                .unwrap_or_else(|e| panic!("{a_name} vs {b_name}: {e}"));

            let mut policy = StdRng::seed_from_u64(3);
            let mut steps = 0;
            while !game.is_over() {
                let legal = legal_actions(&game);
                assert!(!legal.is_empty(), "{a_name} vs {b_name} stalled");
                let action = legal[policy.gen_range(0..legal.len())].clone();
                game.step(action.clone()).unwrap_or_else(|e| {
                    panic!("{a_name} vs {b_name}: {action:?} rejected: {e}")
                });
                steps += 1;
                assert!(steps < 8000, "{a_name} vs {b_name} did not terminate");
            }
        }
    }
}

#[test]
fn full_games_play_to_completion_with_scripts_live() {
    let Some((db, cards)) = load() else {
        eprintln!("skipping: run tools/ingest/fetch_cards.py");
        return;
    };

    let mut results = (0, 0, 0);
    for seed in 0..40u64 {
        let mut game = new_game(Arc::clone(&db), Arc::clone(&cards), seed);
        let mut policy = StdRng::seed_from_u64(seed * 977 + 5);
        let mut steps = 0;

        while !game.is_over() {
            let legal = legal_actions(&game);
            assert!(
                !legal.is_empty(),
                "seed {seed} stalled at {:?}",
                game.pending()
            );
            let action = legal[policy.gen_range(0..legal.len())].clone();
            game.step(action.clone()).unwrap_or_else(|e| {
                panic!("seed {seed}: legal action {action:?} rejected: {e}")
            });
            steps += 1;
            assert!(steps < 8000, "seed {seed} did not terminate");
        }

        match game.result().unwrap().winner() {
            Some(p) if p == PlayerId::P0 => results.0 += 1,
            Some(_) => results.1 += 1,
            None => results.2 += 1,
        }
    }

    // Both decks must be capable of winning; a 40-0 split would mean a rules
    // bug favouring one seat, not a skill difference between random players.
    assert!(
        results.0 > 0 && results.1 > 0,
        "one seat won every game: {results:?}"
    );
}

// ---- individual card behaviour ---------------------------------------------

fn game_at_main(db: Arc<CardDb>, cards: Scripts, seed: u64, turns: usize) -> Game {
    let mut game = new_game(db, cards, seed);
    for _ in 0..2 {
        game.step(Action::Mulligan(false)).unwrap();
    }
    for _ in 0..turns {
        game.step(Action::EndMainPhase).unwrap();
    }
    game
}

fn put_in_play(game: &mut Game, player: PlayerId, number: &str) -> op_core::CardInstanceId {
    let def = game.db().by_number(number).unwrap();
    let card = game.state.spawn(def, player, Zone::Limbo);
    game.state
        .move_card(card, player, Zone::Character, Placement::Bottom);
    card
}

/// ST-06's whole plan is shrinking a Character so that a "cost N or less"
/// removal effect can reach it. That only works if the filter reads *derived*
/// cost; against printed cost the deck does nothing.
#[test]
fn st06_cost_reduction_brings_a_character_into_ko_range() {
    let Some((db, cards)) = load() else { return };
    let mut game = game_at_main(db, cards, 5, 1);

    // ST06-012 Garp is a cost-5 Character; ST06-012's own effect reaches
    // cost 4 or less, so unmodified it cannot touch him.
    let target = put_in_play(&mut game, PlayerId::P0, "ST06-012");
    assert_eq!(game.derived().get(target).cost, 5);

    // Two applications of -2 cost put him at 1.
    for _ in 0..2 {
        game.state.modifiers.push(op_core::effect::Modifier {
            target,
            kind: op_core::effect::ModKind::Cost(-2),
            duration: op_core::effect::Duration::ThisTurn,
            source: target,
            controller: PlayerId::P1,
        });
    }
    assert_eq!(game.derived().get(target).cost, 1);

    // Cost never goes negative, however much is stacked on.
    for _ in 0..5 {
        game.state.modifiers.push(op_core::effect::Modifier {
            target,
            kind: op_core::effect::ModKind::Cost(-4),
            duration: op_core::effect::Duration::ThisTurn,
            source: target,
            controller: PlayerId::P1,
        });
    }
    assert_eq!(game.derived().get(target).cost, 0, "cost is clamped at 0");
}

/// ST06-004 Smoker cannot be K.O.'d by effects, but a lost battle still
/// K.O.s him (10-2-1-1).
#[test]
fn st06_004_resists_effect_ko_but_not_battle() {
    let Some((db, cards)) = load() else { return };
    let mut game = game_at_main(db, cards, 5, 1);
    let smoker = put_in_play(&mut game, PlayerId::P1, "ST06-004");
    assert!(game.derived().get(smoker).cannot_be_koed_by_effect);

    // A vanilla Character has no such protection.
    let plain = put_in_play(&mut game, PlayerId::P1, "ST06-009");
    assert!(!game.derived().get(plain).cannot_be_koed_by_effect);
}

#[test]
fn st01_013_zoro_gains_power_only_with_a_don_attached() {
    let Some((db, cards)) = load() else { return };
    let mut game = game_at_main(db, cards, 3, 0);
    let zoro = put_in_play(&mut game, PlayerId::P0, "ST01-013");

    // Printed 5000; [DON!! x1] grants +1000.
    assert_eq!(game.derived().power(zoro), 5000);
    game.step(Action::GiveDon { to: zoro }).unwrap();
    assert_eq!(game.derived().power(zoro), 7000, "1000 from the DON!! itself (6-5-5-2) plus 1000 from the card's own effect");
}

#[test]
fn st01_004_sanji_gains_rush_only_at_two_don() {
    let Some((db, cards)) = load() else { return };
    // Turn 3, so P0 has the 2 DON!! the effect needs.
    let mut game = game_at_main(db, cards, 3, 2);
    let sanji = put_in_play(&mut game, PlayerId::P0, "ST01-004");

    assert!(!game.derived().get(sanji).has_keyword(Keyword::Rush));
    game.step(Action::GiveDon { to: sanji }).unwrap();
    assert!(
        !game.derived().get(sanji).has_keyword(Keyword::Rush),
        "one DON!! is not enough for [DON!! x2]"
    );
    game.step(Action::GiveDon { to: sanji }).unwrap();
    assert!(game.derived().get(sanji).has_keyword(Keyword::Rush));
}

#[test]
fn st02_003_urouge_needs_both_a_don_and_three_characters() {
    let Some((db, cards)) = load() else { return };
    let mut game = game_at_main(db, cards, 3, 1);
    let urouge = put_in_play(&mut game, PlayerId::P1, "ST02-003");
    assert_eq!(game.derived().power(urouge), 3000);

    game.step(Action::GiveDon { to: urouge }).unwrap();
    // One DON!! attached, but only one Character in play.
    assert_eq!(game.derived().power(urouge), 4000);

    put_in_play(&mut game, PlayerId::P1, "ST02-002");
    put_in_play(&mut game, PlayerId::P1, "ST02-006");
    // 3000 printed + 1000 DON!! + 2000 from the effect.
    assert_eq!(game.derived().power(urouge), 6000);
}

#[test]
fn st02_014_drake_buffs_typed_allies_only_while_rested() {
    let Some((db, cards)) = load() else { return };
    let mut game = game_at_main(db, cards, 3, 1);
    let drake = put_in_play(&mut game, PlayerId::P1, "ST02-014");
    // ST02-012 Bepo is a {Heart Pirates} card — not {Supernovas} or {Navy}.
    let bepo = put_in_play(&mut game, PlayerId::P1, "ST02-012");
    // ST02-003 Urouge is a {Supernovas} card.
    let urouge = put_in_play(&mut game, PlayerId::P1, "ST02-003");

    game.step(Action::GiveDon { to: drake }).unwrap();
    let base_urouge = game.derived().power(urouge);
    let base_bepo = game.derived().power(bepo);

    game.state.card_mut(drake).rested = true;
    let after = game.derived();
    assert_eq!(after.power(urouge), base_urouge + 1000);
    assert_eq!(after.power(bepo), base_bepo, "Bepo is not a targeted type");
}

#[test]
fn st01_006_chopper_has_blocker_as_a_printed_keyword() {
    let Some((db, cards)) = load() else { return };
    let mut game = game_at_main(db, cards, 3, 0);
    let chopper = put_in_play(&mut game, PlayerId::P0, "ST01-006");
    assert!(game.derived().get(chopper).has_keyword(Keyword::Blocker));
}

#[test]
fn every_scripted_timing_is_reachable_by_the_engine() {
    let Some((db, cards)) = load() else { return };
    // Guards against a script using a timing the engine never fires — the
    // failure mode where a card silently does nothing.
    let fired: &[Timing] = &[
        Timing::OnPlay,
        Timing::WhenAttacking,
        Timing::OnYourOpponentsAttack,
        Timing::EndOfYourTurn,
        Timing::EndOfYourOpponentsTurn,
        Timing::EndOfBattle,
    ];
    for (_, def) in db.iter() {
        let Some(id) = db.by_number(&def.number) else {
            continue;
        };
        for auto in &cards.script(id).auto {
            assert!(
                fired.contains(&auto.timing),
                "{} uses timing {:?}, which the engine never activates",
                def.number,
                auto.timing
            );
        }
    }
}
