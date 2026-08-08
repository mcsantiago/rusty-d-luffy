//! End-to-end tests against the real ST-01 and ST-02 card pool.
//!
//! Skipped when `data/` is unpopulated; run
//! `python3 tools/ingest/fetch_cards.py` first.

use std::sync::Arc;

use op_cards::Cards;
use op_core::card::{CardDb, Keyword};
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
        ]),
    }
}

/// The official ST-02 Worst Generation decklist.
fn st02() -> DeckList {
    DeckList {
        leader: "ST02-001".into(),
        cards: counts(&[
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
        ]),
    }
}

/// ST-06 Absolute Justice. A legal 50-card build, not the printed list.
fn st06() -> DeckList {
    DeckList {
        leader: "ST06-001".into(),
        cards: counts(&[
            ("ST06-002", 4),
            ("ST06-003", 4),
            ("ST06-004", 2),
            ("ST06-005", 2),
            ("ST06-006", 4),
            ("ST06-007", 4),
            ("ST06-008", 4),
            ("ST06-009", 4),
            ("ST06-010", 4),
            ("ST06-011", 2),
            ("ST06-012", 2),
            ("ST06-013", 4),
            ("ST06-014", 4),
            ("ST06-015", 2),
            ("ST06-016", 2),
            ("ST06-017", 2),
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
    Game::new(config, db, cards)
        .expect("starter decks must be legal")
        .0
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
    type Build = fn() -> DeckList;
    let decks: [(&str, Build); 3] = [("ST-01", st01), ("ST-02", st02), ("ST-06", st06)];

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
                game.step(action.clone())
                    .unwrap_or_else(|e| panic!("{a_name} vs {b_name}: {action:?} rejected: {e}"));
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
            game.step(action.clone())
                .unwrap_or_else(|e| panic!("seed {seed}: legal action {action:?} rejected: {e}"));
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
    assert_eq!(game.derived().get(target).effective_cost(), 5);

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
    assert_eq!(game.derived().get(target).effective_cost(), 1);

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
    assert_eq!(
        game.derived().get(target).effective_cost(),
        0,
        "cost is clamped at 0"
    );
}

/// ST06-004 Smoker cannot be K.O.'d by effects, but a lost battle still
/// K.O.s him (10-2-1-1).
/// Sakazuki K.O.s a Character "with a cost of 0", and no built-in deck contains
/// one — cost 0 is only reachable after ST-06's own reduction effects. The
/// activation stays legal and still costs, so the UI has to be able to say so
/// up front rather than leaving the player to infer it from a silent board.
#[test]
fn st06_001_reports_when_it_has_no_legal_target() {
    let Some((db, cards)) = load() else { return };
    let mut game = game_at_main(db, cards, 5, 1);

    let leader = game.state.player(PlayerId::P0).leader.unwrap();
    assert_eq!(
        game.db().get(game.state.card(leader).def).number,
        "ST01-001"
    );

    // A cost-1 Character is not a legal target for a "cost of 0" effect.
    let victim = put_in_play(&mut game, PlayerId::P1, "ST06-003");
    assert_eq!(game.derived().get(victim).effective_cost(), 1);

    let sakazuki = game.state.player(PlayerId::P1).leader.unwrap();
    if game.db().get(game.state.card(sakazuki).def).number == "ST06-001" {
        assert!(
            !game.activation_finds_targets(sakazuki, 0),
            "nothing costs 0, so the effect has no target"
        );
    }

    // Shrinking it to 0 makes the same activation meaningful.
    game.state.modifiers.push(op_core::effect::Modifier {
        target: victim,
        kind: op_core::effect::ModKind::Cost(-4),
        duration: op_core::effect::Duration::ThisTurn,
        source: victim,
        controller: PlayerId::P0,
    });
    assert_eq!(game.derived().get(victim).effective_cost(), 0);
}

/// The whole ST-06 loop, end to end: reduce a Character's cost to 0, then
/// K.O. it with an effect that only reaches cost 0.
///
/// Every step has a way to go silently wrong. If `CostAtMost` read printed
/// cost the reduction would be cosmetic; if reduction went negative rather
/// than clamping, a -4 on a 3-cost Character would miss; and the activation
/// has to become available only after the reduction lands.
#[test]
fn st06_reduce_to_zero_then_remove_is_a_working_loop() {
    let Some((db, cards)) = load() else { return };
    let mut game = game_at_main(db, cards, 5, 1);

    // ST06-013 T-Bone costs 3. Sakazuki reaches cost 0 only.
    let victim = put_in_play(&mut game, PlayerId::P1, "ST06-013");
    assert_eq!(game.derived().get(victim).effective_cost(), 3);

    // Sengoku's -4 on a 3-cost Character: clamps to 0 rather than -1, which is
    // what makes the "or more reduction than you need" case work at all.
    game.state.modifiers.push(op_core::effect::Modifier {
        target: victim,
        kind: op_core::effect::ModKind::Cost(-4),
        duration: op_core::effect::Duration::ThisTurn,
        source: victim,
        controller: PlayerId::P0,
    });
    assert_eq!(
        game.derived().get(victim).effective_cost(),
        0,
        "-4 on a 3-cost Character clamps to 0, it does not go negative"
    );

    // And a cost-0 filter now reaches it, which is the point of the whole deck.
    let derived = game.derived();
    assert!(op_core::derive::matches_filters(
        &game.state,
        game.db(),
        &derived,
        victim,
        victim,
        &[op_core::effect::Filter::CostAtMost(0)],
    ));

    // A Character that has not been reduced is still out of reach.
    let untouched = put_in_play(&mut game, PlayerId::P1, "ST06-011");
    let derived = game.derived();
    assert!(!op_core::derive::matches_filters(
        &game.state,
        game.db(),
        &derived,
        untouched,
        untouched,
        &[op_core::effect::Filter::CostAtMost(0)],
    ));

    // The reduction is "during this turn" and must not outlive it.
    game.step(Action::EndMainPhase).unwrap();
    while game.state.turn < 3 {
        if game.step(Action::EndMainPhase).is_err() {
            break;
        }
    }
    assert_eq!(
        game.derived().get(victim).effective_cost(),
        3,
        "cost reduction expires with the turn (6-6-1-3)"
    );
}

/// 1-3: a negative cost keeps its value for the duration of a calculation and
/// is only treated as 0 outside one. Clamping each modifier as it applied
/// would lose that — a 3-cost Character given -4 then +2 would read 2 rather
/// than 1.
#[test]
fn negative_cost_survives_the_calculation_and_clamps_only_on_read() {
    let Some((db, cards)) = load() else { return };
    let mut game = game_at_main(db, cards, 5, 1);
    let card = put_in_play(&mut game, PlayerId::P1, "ST06-013"); // cost 3

    let modifier = |amount: i32| op_core::effect::Modifier {
        target: card,
        kind: op_core::effect::ModKind::Cost(amount),
        duration: op_core::effect::Duration::ThisTurn,
        source: card,
        controller: PlayerId::P0,
    };

    game.state.modifiers.push(modifier(-4));
    let derived = game.derived();
    assert_eq!(derived.get(card).cost, -1, "the raw value goes negative");
    assert_eq!(derived.get(card).effective_cost(), 0, "and reads as 0");

    game.state.modifiers.push(modifier(2));
    let derived = game.derived();
    assert_eq!(
        derived.get(card).effective_cost(),
        1,
        "3 - 4 + 2 is 1; clamping per modifier would give 2"
    );
}

/// ST06-015 Great Eruption draws a card and then targets. Playing it with an
/// empty opposing board still draws, but the targeting clause has nowhere to
/// go — and it is played, not activated, so the warning has to reach Events
/// and not only [Activate: Main] effects.
#[test]
fn st06_015_warns_when_its_targeting_clause_has_nowhere_to_go() {
    let Some((db, cards)) = load() else { return };
    let mut game = game_at_main(db, cards, 5, 1);

    let def = game.db().by_number("ST06-015").unwrap();
    let card = game.state.spawn(def, PlayerId::P0, Zone::Hand);

    // Nothing on the opposing board: the -2 cost clause has no target.
    assert!(game.state.player(PlayerId::P1).characters.is_empty());
    assert!(!game.play_finds_targets(card));

    // Give the opponent something and the same card becomes fully live.
    put_in_play(&mut game, PlayerId::P1, "ST06-013");
    assert!(game.play_finds_targets(card));
}

/// A card with no targeting text at all must never be flagged.
#[test]
fn a_card_that_targets_nothing_is_never_reported_as_targetless() {
    let Some((db, cards)) = load() else { return };
    let mut game = game_at_main(db, cards, 5, 1);

    for number in ["ST06-003", "ST06-009", "ST06-016"] {
        let def = game.db().by_number(number).unwrap();
        let card = game.state.spawn(def, PlayerId::P0, Zone::Hand);
        assert!(
            game.play_finds_targets(card),
            "{number} asks for no target, so it must not be flagged"
        );
    }
}

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
    assert_eq!(
        game.derived().power(zoro),
        7000,
        "1000 from the DON!! itself (6-5-5-2) plus 1000 from the card's own effect"
    );
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

// Timing reachability used to be checked here. It is now one of the checks in
// `op_core::validate`, exercised by `tests/scripts_are_well_formed.rs` — which
// needs no `data/`, so it cannot skip itself.
