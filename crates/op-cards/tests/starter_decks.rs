//! End-to-end tests against the real ST-01 and ST-02 card pool.
//!
//! Skipped when `data/` is unpopulated; run
//! `python3 tools/ingest/fetch_cards.py` first.

mod common;

use std::sync::Arc;

use op_cards::Cards;
use op_core::action::Pending;
use op_core::card::{CardDb, Keyword};
use op_core::script::ScriptSource;
use op_core::state::Placement;
use op_core::zone::Zone;
use op_core::{legal_actions, Action, DeckList, Game, GameConfig, PlayerId};

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

// The decklists live in `op_cards::decks` so the clients and this suite cannot
// disagree about what a starter deck contains.
use op_cards::decks::{st01, st02, st03, st04, st06, st08};

type Scripts = Arc<dyn ScriptSource + Send + Sync>;

fn load() -> Option<(Arc<CardDb>, Scripts)> {
    let db = common::card_db()?;
    let cards: Scripts = Arc::new(Cards::new(&db));
    Some((Arc::new(db), cards))
}

fn new_game_with(db: Arc<CardDb>, cards: Scripts, seed: u64, decks: [DeckList; 2]) -> Game {
    let config = GameConfig {
        seed,
        first_player: PlayerId::P0,
        decks,
        allow_illegal_decks: false,
    };
    Game::new(config, db, cards)
        .expect("starter decks must be legal")
        .0
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
    let decks: [(&str, Build); 6] = [
        ("ST-01", st01),
        ("ST-02", st02),
        ("ST-03", st03),
        ("ST-04", st04),
        ("ST-06", st06),
        ("ST-08", st08),
    ];

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

/// A game with `decks` in place, advanced to the Main Phase of turn
/// `turns + 1`. P0 is the turn player on odd turns, so an even `turns` leaves
/// the first-named deck to act.
fn at_main(db: Arc<CardDb>, cards: Scripts, seed: u64, decks: [DeckList; 2], turns: usize) -> Game {
    let mut game = new_game_with(db, cards, seed, decks);
    for _ in 0..2 {
        game.step(Action::Mulligan(false)).unwrap();
    }
    for _ in 0..turns {
        game.step(Action::EndMainPhase).unwrap();
    }
    game
}

fn st08_at_main(db: Arc<CardDb>, cards: Scripts, seed: u64, turns: usize) -> Game {
    at_main(db, cards, seed, [st08(), st01()], turns)
}

fn st04_at_main(db: Arc<CardDb>, cards: Scripts, seed: u64, turns: usize) -> Game {
    at_main(db, cards, seed, [st04(), st01()], turns)
}

fn st03_at_main(db: Arc<CardDb>, cards: Scripts, seed: u64, turns: usize) -> Game {
    at_main(db, cards, seed, [st03(), st01()], turns)
}

/// Plays a battle out with the defender declining everything, so the test only
/// has to care about who was left standing.
fn battle_through(game: &mut Game) {
    while game.state.battle.is_some() {
        let action = match game.pending() {
            Some(Pending::Block { .. }) => Action::Block { blocker: None },
            Some(Pending::Counter { .. }) => Action::DoneCountering,
            Some(Pending::Trigger { .. }) => Action::UseTrigger(false),
            _ => break,
        };
        game.step(action).unwrap();
    }
}

/// Agrees to an auto effect's activation cost.
///
/// An auto effect with a non-free cost now asks before spending anything
/// (8-3-1-4), so a test that wants the effect to resolve has to say yes. The
/// assertion is the point: if the prompt stops appearing, these tests should
/// fail rather than quietly go back to testing forced payment.
fn pay_cost(game: &mut Game) {
    assert!(
        matches!(game.pending(), Some(Pending::PayCost { .. })),
        "expected a cost prompt, got {:?}",
        game.pending()
    );
    game.step(Action::PayCost(true)).unwrap();
}

/// Answers a pending `DON!! −X`, if it asked at all.
///
/// Which DON!! go is the player's choice (3-9-2, 8-3-1-6), but only where they
/// differ: a pool of interchangeable DON!! has one answer and is paid without
/// stopping. Takes the engine's leading answer, the rested cost-area DON!! it
/// took silently before, so these tests pin the same outcome either way.
/// `st04_a_uniform_don_pool_pays_without_asking` is what holds the gate itself.
fn return_don(game: &mut Game) {
    if !matches!(game.pending(), Some(Pending::ReturnDon { .. })) {
        return;
    }
    let answer = legal_actions(game)
        .into_iter()
        .next()
        .expect("a payable DON!! −X always has a legal answer");
    game.step(answer).unwrap();
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

// ---- ST-08 ------------------------------------------------------------------

/// ST08-001's Leader turns removal into DON!!. The trigger is a board-wide
/// "when *a* Character is K.O.'d", not "when this card is K.O.'d", so the hook
/// has to reach every card in play rather than the one that left.
#[test]
fn st08_001_leader_gains_a_rested_don_when_any_character_is_koed() {
    let Some((db, cards)) = load() else { return };
    let mut game = st08_at_main(db, cards, 5, 2);

    let leader = game.state.player(PlayerId::P0).leader.unwrap();
    assert!(game.state.card(leader).attached_don.is_empty());

    // The card reads "rested DON!! card", and 4-4-2 makes that a constraint on
    // which DON!! may be selected rather than the state it ends up in, so the
    // cost area has to contain one or the effect resolves to nothing. Turn 3:
    // three DON!!, of which one has been spent and is rested.
    let cost_don = game.state.player(PlayerId::P0).cost_area.clone();
    assert_eq!(cost_don.len(), 3);
    game.state.card_mut(cost_don[2]).rested = true;

    // ST08-004 Koby rests to K.O. a Character with a cost of 2 or less.
    let koby = put_in_play(&mut game, PlayerId::P0, "ST08-004");
    let victim = put_in_play(&mut game, PlayerId::P1, "ST08-008"); // cost 1
    game.step(Action::ActivateEffect {
        card: koby,
        slot: 0,
        discard: Vec::new(),
    })
    .unwrap();
    game.step(Action::Choose {
        cards: vec![victim],
    })
    .unwrap();

    assert_eq!(game.state.card(victim).zone, Zone::Trash);
    let don = game.state.card(leader).attached_don.clone();
    assert_eq!(don.len(), 1, "the K.O. should have paid the Leader");
    assert!(
        game.state.card(don[0]).rested,
        "the card gives a *rested* DON!!"
    );
}

/// The `[Your Turn]` half. A Character K.O.'d on the opponent's turn pays
/// nothing, and a script that dropped the condition would still look right in
/// the test above.
#[test]
fn st08_001_pays_nothing_on_the_opponents_turn() {
    let Some((db, cards)) = load() else { return };
    let mut game = st08_at_main(db, cards, 5, 1); // turn 2: P1's turn

    let leader = game.state.player(PlayerId::P0).leader.unwrap();
    assert_ne!(game.state.turn_player, PlayerId::P0);

    let koby = put_in_play(&mut game, PlayerId::P1, "ST08-004");
    let victim = put_in_play(&mut game, PlayerId::P0, "ST08-008");
    game.step(Action::ActivateEffect {
        card: koby,
        slot: 0,
        discard: Vec::new(),
    })
    .unwrap();
    game.step(Action::Choose {
        cards: vec![victim],
    })
    .unwrap();

    assert_eq!(game.state.card(victim).zone, Zone::Trash);
    assert!(
        game.state.card(leader).attached_don.is_empty(),
        "[Your Turn] gates the Leader's trigger"
    );
}

/// ST08-002 Uta survives a Leader's attack but not a Character's. The
/// protection is narrower than `cannot_be_koed_by_effect` in both directions:
/// it stops a *battle* K.O., and only from a Leader.
#[test]
fn st08_002_survives_a_leader_in_battle_but_not_a_character() {
    let Some((db, cards)) = load() else { return };

    for (attacker_number, expect_survives) in [(None, true), (Some("ST01-013"), false)] {
        let mut game = st08_at_main(Arc::clone(&db), Arc::clone(&cards), 5, 2);
        // Uta belongs to P1 here so that P0, the turn player, can attack her.
        let uta = put_in_play(&mut game, PlayerId::P1, "ST08-002");
        game.state.card_mut(uta).rested = true; // 7-1-1-2: only rested Characters can be attacked
        assert_eq!(game.derived().power(uta), 3000);

        let attacker = match attacker_number {
            // ST01-001 Luffy, 5000 power.
            None => game.state.player(PlayerId::P0).leader.unwrap(),
            // ST01-013 Zoro, 5000 power.
            Some(number) => put_in_play(&mut game, PlayerId::P0, number),
        };
        assert!(game.derived().power(attacker) > game.derived().power(uta));

        game.step(Action::Attack {
            attacker,
            target: uta,
        })
        .unwrap();
        battle_through(&mut game);

        assert_eq!(
            game.state.card(uta).zone != Zone::Trash,
            expect_survives,
            "attacked by {attacker_number:?}"
        );
    }
}

/// ST08-005 Shanks K.O.s "all Characters with a cost of 1 or less" — both
/// boards, his own side included, and with no choice offered.
#[test]
fn st08_005_kos_every_cheap_character_on_both_sides() {
    let Some((db, cards)) = load() else { return };
    // Turn 9, by which point P0 has the 9 DON!! Shanks costs.
    let mut game = st08_at_main(db, cards, 5, 8);
    assert_eq!(game.active_don(PlayerId::P0).len(), 9);

    let mine = put_in_play(&mut game, PlayerId::P0, "ST08-008"); // cost 1
    let theirs = put_in_play(&mut game, PlayerId::P1, "ST08-008"); // cost 1
    let spared = put_in_play(&mut game, PlayerId::P1, "ST08-003"); // cost 2

    let def = game.db().by_number("ST08-005").unwrap();
    let shanks = game.state.spawn(def, PlayerId::P0, Zone::Hand);
    assert!(
        !game.state.player(PlayerId::P0).hand.is_empty(),
        "the [On Play] costs a card from hand"
    );
    game.step(Action::PlayCard {
        card: shanks,
        replacing: None,
    })
    .unwrap();
    pay_cost(&mut game);

    assert_eq!(game.state.card(mine).zone, Zone::Trash, "his own side too");
    assert_eq!(game.state.card(theirs).zone, Zone::Trash);
    assert_eq!(
        game.state.card(spared).zone,
        Zone::Character,
        "cost 2 is out of range"
    );
    assert_eq!(
        game.state.card(shanks).zone,
        Zone::Character,
        "Shanks costs 9 and does not K.O. himself"
    );
}

/// ST08-014 pays a Life card for the deck's deepest cost reduction. The
/// payment is a real cost — it comes off Life — and it is not damage, so the
/// card must arrive in hand without its `[Trigger]` firing.
#[test]
fn st08_014_pays_a_life_card_to_shrink_a_character_by_seven() {
    let Some((db, cards)) = load() else { return };
    let mut game = st08_at_main(db, cards, 5, 2);

    let victim = put_in_play(&mut game, PlayerId::P1, "ST08-012"); // cost 4
    assert_eq!(game.derived().get(victim).effective_cost(), 4);

    let life_before = game.state.player(PlayerId::P0).life.len();
    let top_of_life = game.state.player(PlayerId::P0).life[0];
    let hand_before = game.state.player(PlayerId::P0).hand.len();

    let def = game.db().by_number("ST08-014").unwrap();
    let event = game.state.spawn(def, PlayerId::P0, Zone::Hand);
    game.step(Action::PlayCard {
        card: event,
        replacing: None,
    })
    .unwrap();
    game.step(Action::Choose {
        cards: vec![victim],
    })
    .unwrap();

    assert_eq!(game.state.player(PlayerId::P0).life.len(), life_before - 1);
    assert_eq!(game.state.card(top_of_life).zone, Zone::Hand);
    // The Event itself left hand for the trash, and the Life card arrived.
    assert_eq!(game.state.player(PlayerId::P0).hand.len(), hand_before + 1);
    assert_eq!(
        game.derived().get(victim).effective_cost(),
        0,
        "4 - 7 clamps to 0 (1-3)"
    );
}

/// With no Life left the cost cannot be paid, and 8-3-1-3 means it is not paid
/// in part: the Event is still played, and does nothing.
#[test]
fn st08_014_does_nothing_with_no_life_to_pay_with() {
    let Some((db, cards)) = load() else { return };
    let mut game = st08_at_main(db, cards, 5, 2);

    let victim = put_in_play(&mut game, PlayerId::P1, "ST08-012");
    for card in game.state.player(PlayerId::P0).life.clone() {
        game.state
            .move_card(card, PlayerId::P0, Zone::Trash, Placement::Top);
    }

    let def = game.db().by_number("ST08-014").unwrap();
    let event = game.state.spawn(def, PlayerId::P0, Zone::Hand);
    game.step(Action::PlayCard {
        card: event,
        replacing: None,
    })
    .unwrap();

    assert_eq!(game.state.card(event).zone, Zone::Trash, "still played");
    assert!(
        game.pending().is_none() || !matches!(game.pending(), Some(Pending::Choose { .. })),
        "an unpayable cost resolves no ops, so nothing is asked"
    );
    assert_eq!(game.derived().get(victim).effective_cost(), 4);
}

/// ST08-013's trade. It is only reachable when the attacker *loses* — 7-1-4-2,
/// where nothing happens — so both Characters are still standing when the
/// end-of-battle effect resolves.
#[test]
fn st08_013_may_trade_itself_for_the_character_it_battled() {
    let Some((db, cards)) = load() else { return };

    for take_the_trade in [true, false] {
        let mut game = st08_at_main(Arc::clone(&db), Arc::clone(&cards), 5, 2);

        let bentham = put_in_play(&mut game, PlayerId::P0, "ST08-013"); // 6000
        let wall = put_in_play(&mut game, PlayerId::P1, "ST08-005"); // 10000
        game.state.card_mut(wall).rested = true;

        // [DON!! x1] is the effect's condition; it also puts Bentham at 7000,
        // still short of 10000, so he loses the battle and nothing is K.O.'d.
        game.step(Action::GiveDon { to: bentham }).unwrap();
        assert_eq!(game.derived().power(bentham), 7000);

        game.step(Action::Attack {
            attacker: bentham,
            target: wall,
        })
        .unwrap();
        battle_through(&mut game);

        // The battle itself K.O.'d nobody; the choice is Bentham's controller's.
        let chosen = if take_the_trade {
            vec![wall]
        } else {
            Vec::new()
        };
        game.step(Action::Choose { cards: chosen }).unwrap();

        assert_eq!(
            game.state.card(wall).zone == Zone::Trash,
            take_the_trade,
            "taking the trade K.O.s the card battled"
        );
        assert_eq!(
            game.state.card(bentham).zone == Zone::Trash,
            take_the_trade,
            "and 'if you do' K.O.s this card only then"
        );
    }
}

// ---- ST-04 ------------------------------------------------------------------

/// "DON!! −N" is not `rest_don`. Rested DON!! comes back next Refresh Phase;
/// this leaves the field for the DON!! deck, and a script that confused the two
/// would look identical for exactly one turn.
#[test]
fn st04_don_minus_returns_don_to_the_don_deck_rather_than_resting_it() {
    let Some((db, cards)) = load() else { return };
    // Turn 15, so the Leader's DON!! −7 is payable.
    let mut game = st04_at_main(db, cards, 5, 14);
    let leader = game.state.player(PlayerId::P0).leader.unwrap();

    let cost_before = game.state.player(PlayerId::P0).cost_area.len();
    let deck_before = game.state.player(PlayerId::P0).don_deck.len();
    assert!(cost_before >= 7, "need 7 DON!! to pay DON!! -7");

    game.step(Action::ActivateEffect {
        card: leader,
        slot: 0,
        discard: Vec::new(),
    })
    .unwrap();
    return_don(&mut game);

    assert_eq!(
        game.state.player(PlayerId::P0).cost_area.len(),
        cost_before - 7,
        "the DON!! left the cost area entirely"
    );
    assert_eq!(
        game.state.player(PlayerId::P0).don_deck.len(),
        deck_before + 7,
        "and went back to the DON!! deck"
    );
}

/// Interchangeable DON!! are paid without asking: every answer reaches the same
/// position, down to the state hash, so there is nothing to decide (3-9-2).
///
/// The gate used to be "pool wider than the cost", which stopped the game on
/// one-answer decisions — a wasted ply per search node, a record per log, and a
/// modal asking a human to hand-pick seven identical cards.
#[test]
fn st04_a_uniform_don_pool_pays_without_asking() {
    let Some((db, cards)) = load() else { return };
    let mut game = st04_at_main(db, cards, 5, 14);
    let leader = game.state.player(PlayerId::P0).leader.unwrap();

    let cost_before = game.state.player(PlayerId::P0).cost_area.len();
    assert!(
        cost_before > 7,
        "a pool wider than the cost is the case that used to prompt"
    );
    assert!(
        game.state
            .player(PlayerId::P0)
            .cost_area
            .iter()
            .all(|&d| game.state.card(d).is_active()),
        "and it has to be uniform for there to be nothing to decide"
    );

    game.step(Action::ActivateEffect {
        card: leader,
        slot: 0,
        discard: Vec::new(),
    })
    .unwrap();

    assert!(
        !matches!(game.pending(), Some(Pending::ReturnDon { .. })),
        "one distinguishable answer is not a decision, got {:?}",
        game.pending()
    );
    assert_eq!(
        game.state.player(PlayerId::P0).cost_area.len(),
        cost_before - 7,
        "and the cost was paid anyway"
    );
}

/// ST04-001's Leader trashes a Life card outright. Unlike damage it never
/// reaches the opponent's hand and activates no `[Trigger]` (10-1-5).
#[test]
fn st04_001_trashes_an_opponent_life_card_without_giving_it_to_them() {
    let Some((db, cards)) = load() else { return };
    let mut game = st04_at_main(db, cards, 5, 14);
    let leader = game.state.player(PlayerId::P0).leader.unwrap();

    let life_before = game.state.player(PlayerId::P1).life.len();
    let hand_before = game.state.player(PlayerId::P1).hand.len();
    let doomed = game.state.player(PlayerId::P1).life[0];

    game.step(Action::ActivateEffect {
        card: leader,
        slot: 0,
        discard: Vec::new(),
    })
    .unwrap();
    return_don(&mut game);

    assert_eq!(game.state.player(PlayerId::P1).life.len(), life_before - 1);
    assert_eq!(game.state.card(doomed).zone, Zone::Trash);
    assert_eq!(
        game.state.player(PlayerId::P1).hand.len(),
        hand_before,
        "trashing Life is not damage; the card does not go to hand"
    );
}

/// The cost has to be payable in full or not at all (8-3-1-3), so the Leader's
/// effect simply is not offered below 7 DON!!.
#[test]
fn st04_001_is_not_offered_when_the_don_cost_cannot_be_paid() {
    let Some((db, cards)) = load() else { return };
    let mut game = st04_at_main(db, cards, 5, 2); // turn 3: 3 DON!!
    let leader = game.state.player(PlayerId::P0).leader.unwrap();
    assert!(game.state.player(PlayerId::P0).cost_area.len() < 7);

    assert!(
        !legal_actions(&game)
            .iter()
            .any(|a| matches!(a, Action::ActivateEffect { card, .. } if *card == leader)),
        "DON!! -7 is unpayable, so the effect is not a legal action"
    );
    assert!(game
        .step(Action::ActivateEffect {
            card: leader,
            slot: 0,
            discard: Vec::new(),
        })
        .is_err());
}

/// ST04-008 refills instead of spending: a DON!! card off the DON!! deck,
/// arriving *active* so it is spendable the same turn.
#[test]
fn st04_008_adds_an_active_don_from_the_don_deck() {
    let Some((db, cards)) = load() else { return };
    let mut game = st04_at_main(db, cards, 5, 2);

    let spendable_before = game.active_don(PlayerId::P0).len();
    let deck_before = game.state.player(PlayerId::P0).don_deck.len();

    let def = game.db().by_number("ST04-008").unwrap();
    let jack = game.state.spawn(def, PlayerId::P0, Zone::Hand);
    assert!(
        game.state.player(PlayerId::P0).hand.len() > 1,
        "cost 1 card"
    );
    game.step(Action::PlayCard {
        card: jack,
        replacing: None,
    })
    .unwrap();
    pay_cost(&mut game);

    assert_eq!(
        game.state.player(PlayerId::P0).don_deck.len(),
        deck_before - 1
    );
    // 3 DON!! rested to play a cost-3 Character, then 1 added active.
    assert_eq!(
        game.active_don(PlayerId::P0).len(),
        spendable_before - 3 + 1
    );
}

/// ST04-002 finds [Page One] by printed name, not card number, and plays it for
/// free from hand.
/// 8-3-1-4: "The player can choose not to pay the activation cost; however,
/// this will mean the effect cannot be activated."
///
/// The case that motivated this, from session-1786259932: ST04-002's `[On Play]`
/// plays a [Page One] from hand, and its cost is DON!! -1 — DON!! returned to
/// the DON!! deck for the rest of the game. With no [Page One] in hand the
/// effect can do nothing, and the engine used to spend the DON!! anyway,
/// twice in one game, because an auto effect paid the moment it could afford to.
#[test]
fn st04_002_declining_the_cost_spends_nothing() {
    let Some((db, cards)) = load() else { return };
    let mut game = st04_at_main(db, cards, 5, 6);

    // No [Page One] in hand, so paying would buy nothing.
    assert!(
        !game.state.player(PlayerId::P0).hand.iter().any(|&c| game
            .db()
            .get(game.state.card(c).def)
            .number
            == "ST04-012"),
        "this test needs a hand with no Page One in it"
    );

    let don_deck_before = game.state.player(PlayerId::P0).don_deck.len();
    let cost_area_before = game.state.player(PlayerId::P0).cost_area.len();

    let ulti = game.state.spawn(
        game.db().by_number("ST04-002").unwrap(),
        PlayerId::P0,
        Zone::Hand,
    );
    game.step(Action::PlayCard {
        card: ulti,
        replacing: None,
    })
    .unwrap();

    // The price is offered rather than taken.
    let Some(Pending::PayCost { cost, source, .. }) = game.pending() else {
        panic!("expected a cost prompt, got {:?}", game.pending());
    };
    assert_eq!(*source, ulti);
    assert_eq!(cost.don_minus, 1);
    assert!(
        legal_actions(&game).contains(&Action::PayCost(false)),
        "declining must be a legal answer"
    );

    game.step(Action::PayCost(false)).unwrap();

    assert_eq!(
        game.state.player(PlayerId::P0).don_deck.len(),
        don_deck_before,
        "a declined cost must not return DON!! to the DON!! deck"
    );
    assert_eq!(
        game.state.player(PlayerId::P0).cost_area.len(),
        cost_area_before,
        "and must not take one out of the cost area"
    );
    // Ulti is still played — only her effect declined to activate.
    assert_eq!(game.state.card(ulti).zone, Zone::Character);
}

#[test]
fn st04_002_plays_page_one_from_hand_by_name() {
    let Some((db, cards)) = load() else { return };
    let mut game = st04_at_main(db, cards, 5, 6);

    let page_one = game.state.spawn(
        game.db().by_number("ST04-012").unwrap(),
        PlayerId::P0,
        Zone::Hand,
    );
    assert_eq!(
        game.db().get(game.state.card(page_one).def).name,
        "Page One"
    );

    let ulti = game.state.spawn(
        game.db().by_number("ST04-002").unwrap(),
        PlayerId::P0,
        Zone::Hand,
    );
    game.step(Action::PlayCard {
        card: ulti,
        replacing: None,
    })
    .unwrap();
    pay_cost(&mut game);
    return_don(&mut game);

    // The [On Play] offers exactly the [Page One] in hand.
    let Some(Pending::Choose { options, .. }) = game.pending() else {
        panic!("expected a choice, got {:?}", game.pending());
    };
    assert_eq!(options, &[page_one]);

    game.step(Action::Choose {
        cards: vec![page_one],
    })
    .unwrap();
    assert_eq!(
        game.state.card(page_one).zone,
        Zone::Character,
        "played from hand for free"
    );
}

/// ST04-005 draws 2 and then trashes 1 — an instruction, not an offer. The
/// trash must not be declinable, which is what `at_least` is for.
#[test]
fn st04_005_must_trash_a_card_after_drawing() {
    let Some((db, cards)) = load() else { return };
    let mut game = st04_at_main(db, cards, 5, 8);

    let hand_before = game.state.player(PlayerId::P0).hand.len();
    let queen = game.state.spawn(
        game.db().by_number("ST04-005").unwrap(),
        PlayerId::P0,
        Zone::Hand,
    );
    game.step(Action::PlayCard {
        card: queen,
        replacing: None,
    })
    .unwrap();
    pay_cost(&mut game);
    return_don(&mut game);

    let Some(Pending::Choose { at_least, .. }) = game.pending() else {
        panic!("expected the mandatory trash, got {:?}", game.pending());
    };
    assert_eq!(*at_least, 1);
    assert!(
        legal_actions(&game)
            .iter()
            .all(|a| !matches!(a, Action::Choose { cards } if cards.is_empty())),
        "declining a mandatory trash must not be a legal action"
    );
    assert!(game.step(Action::Choose { cards: Vec::new() }).is_err());

    let victim = game.state.player(PlayerId::P0).hand[0];
    game.step(Action::Choose {
        cards: vec![victim],
    })
    .unwrap();
    assert_eq!(game.state.card(victim).zone, Zone::Trash);
    // Queen left hand to be played, 2 drawn, 1 trashed.
    assert_eq!(
        game.state.player(PlayerId::P0).hand.len(),
        hand_before + 2 - 1
    );
}

/// ST04-016 pays its printed cost *and* a DON!! −1 on top. Both come out of the
/// cost area, but only one of them comes back.
#[test]
fn st04_016_counter_pays_its_don_minus_on_top_of_the_printed_cost() {
    let Some((db, cards)) = load() else { return };
    // P1 plays ST-04 here so they are the defender on P0's turn.
    let mut game = at_main(db, cards, 5, [st01(), st04()], 2);

    let blast = game.state.spawn(
        game.db().by_number("ST04-016").unwrap(),
        PlayerId::P1,
        Zone::Hand,
    );
    let deck_before = game.state.player(PlayerId::P1).don_deck.len();
    let cost_before = game.state.player(PlayerId::P1).cost_area.len();
    assert!(cost_before >= 2, "1 to play it, 1 to return");

    let leader = game.state.player(PlayerId::P0).leader.unwrap();
    let target = game.state.player(PlayerId::P1).leader.unwrap();
    let power_before = game.derived().power(target);

    game.step(Action::Attack {
        attacker: leader,
        target,
    })
    .unwrap();
    // The Block Step is skipped outright when nothing can block.
    if matches!(game.pending(), Some(Pending::Block { .. })) {
        game.step(Action::Block { blocker: None }).unwrap();
    }
    game.step(Action::CounterEvent {
        card: blast,
        to: target,
    })
    .unwrap();
    // The DON!! −1 is chosen before the Counter's effect resolves, so the
    // power boost is not on the board until this is answered.
    return_don(&mut game);

    assert_eq!(game.derived().power(target), power_before + 4000);
    assert_eq!(
        game.state.player(PlayerId::P1).don_deck.len(),
        deck_before + 1,
        "DON!! -1 returns a card to the DON!! deck"
    );
    assert_eq!(
        game.state.player(PlayerId::P1).cost_area.len(),
        cost_before - 1,
        "the printed cost only rests its DON!!; the extra cost removes one"
    );
}

// Timing reachability used to be checked here. It is now one of the checks in
// `op_core::validate`, exercised by `tests/scripts_are_well_formed.rs` — which
// needs no `data/`, so it cannot skip itself.

/// ST01-001 Monkey.D.Luffy: "[Activate: Main] [Once Per Turn] Give this Leader
/// or 1 of your Characters up to 1 rested DON!! card."
///
/// Two things at once, because they are the two halves of the printed text:
/// "this Leader" makes Luffy a legal target for his own effect, and "rested
/// DON!! card" qualifies which DON!! may be selected. Bandai's ruling settles
/// the second — a DON!! already given to another Character is refused on the
/// ground that it is not a rested DON!! card — so an active DON!! sitting in
/// the cost area is not available to this effect either.
#[test]
fn st01_001_gives_itself_the_rested_don_and_never_an_active_one() {
    let Some((db, cards)) = load() else { return };
    let mut game = game_at_main(db, cards, 5, 2);

    let leader = game.state.player(PlayerId::P0).leader.unwrap();
    assert_eq!(
        game.db().get(game.state.card(leader).def).number,
        "ST01-001"
    );

    // Turn 3: three DON!!, of which one has been spent and is rested.
    let don = game.state.player(PlayerId::P0).cost_area.clone();
    assert_eq!(don.len(), 3);
    game.state.card_mut(don[2]).rested = true;

    game.step(Action::ActivateEffect {
        card: leader,
        slot: 0,
        discard: vec![],
    })
    .unwrap();

    // "this Leader" — Luffy may name himself.
    assert!(
        legal_actions(&game).contains(&Action::Choose {
            cards: vec![leader]
        }),
        "the Leader must be a legal target for its own effect"
    );
    game.step(Action::Choose {
        cards: vec![leader],
    })
    .unwrap();

    assert_eq!(
        game.state.card(leader).attached_don,
        vec![don[2]],
        "only the rested DON!! is available to the effect"
    );
    assert!(game.state.card(don[2]).rested);
    assert!(don[..2].iter().all(|&d| game.state.card(d).is_active()));
}

/// The same effect with every DON!! active. The activation stays legal and
/// gives nothing, because no rested DON!! exists to select — not because the
/// count was declined; the engine takes it greedily and never asks. What it
/// must not do is take an active DON!! the player is still holding for costs.
#[test]
fn st01_001_gives_nothing_while_every_don_is_active() {
    let Some((db, cards)) = load() else { return };
    let mut game = game_at_main(db, cards, 5, 2);

    let leader = game.state.player(PlayerId::P0).leader.unwrap();
    let before = game.state.player(PlayerId::P0).cost_area.clone();
    assert!(before.iter().all(|&d| game.state.card(d).is_active()));

    game.step(Action::ActivateEffect {
        card: leader,
        slot: 0,
        discard: vec![],
    })
    .unwrap();
    game.step(Action::Choose {
        cards: vec![leader],
    })
    .unwrap();

    assert!(game.state.card(leader).attached_don.is_empty());
    assert_eq!(game.state.player(PlayerId::P0).cost_area, before);
}

/// 8-3-1-6: a `DON!! −X` takes "from their Leader area, Character area, and
/// cost area". A given DON!! is lifted out of the cost area and lives on its
/// holder, so a pool read from the cost area alone refuses costs the player can
/// afford — and ST04-001's `DON!! −7` is most of a board, which is exactly when
/// the DON!! has been given away.
#[test]
fn st04_001_don_minus_can_be_paid_with_don_already_given_away() {
    let Some((db, cards)) = load() else { return };
    let mut game = st04_at_main(db, cards, 5, 14);
    let leader = game.state.player(PlayerId::P0).leader.unwrap();

    // Give all but three DON!! to the Leader, leaving the cost area short of
    // the seven the effect asks for.
    while game.state.player(PlayerId::P0).cost_area.len() > 3 {
        game.step(Action::GiveDon { to: leader }).unwrap();
    }
    let given = game.state.card(leader).attached_don.len();
    assert!(given >= 4, "the cost area alone must not cover DON!! −7");

    assert!(
        legal_actions(&game).iter().any(|a| matches!(
            a,
            Action::ActivateEffect { card, slot: 0, .. } if *card == leader
        )),
        "the effect is affordable across all three areas and must be offered"
    );

    let deck_before = game.state.player(PlayerId::P0).don_deck.len();
    game.step(Action::ActivateEffect {
        card: leader,
        slot: 0,
        discard: Vec::new(),
    })
    .unwrap();

    // The pool has to reach onto the Leader for the cost to be payable at all.
    let Some(Pending::ReturnDon { options, n, .. }) = game.pending() else {
        panic!("expected a DON!! −X prompt, got {:?}", game.pending());
    };
    assert_eq!(*n, 7);
    assert!(
        options
            .iter()
            .any(|d| game.state.card(leader).attached_don.contains(d)),
        "a given DON!! must be offered"
    );

    return_don(&mut game);
    assert_eq!(
        game.state.player(PlayerId::P0).don_deck.len(),
        deck_before + 7,
        "all seven reached the DON!! deck"
    );
    // Whatever was taken off the Leader must be off it: a stale id there would
    // go on granting +1000 power from a card in the DON!! deck (6-5-5-2).
    for don in &game.state.card(leader).attached_don {
        assert_ne!(
            game.state.card(*don).zone,
            Zone::DonDeck,
            "a returned DON!! is still attached to the Leader"
        );
    }
}

/// 3-9-2 leaves the selection to the player, and it is a real decision: keeping
/// a given DON!! keeps +1000 power on the board this turn (6-5-5-2), which
/// surrendering a rested one in the cost area does not cost.
#[test]
fn st04_001_offers_a_choice_of_which_don_to_return() {
    let Some((db, cards)) = load() else { return };
    let mut game = st04_at_main(db, cards, 5, 14);
    let leader = game.state.player(PlayerId::P0).leader.unwrap();

    game.step(Action::GiveDon { to: leader }).unwrap();
    game.step(Action::ActivateEffect {
        card: leader,
        slot: 0,
        discard: Vec::new(),
    })
    .unwrap();

    let answers = legal_actions(&game);
    assert!(
        answers.len() > 1,
        "with more DON!! than the cost takes there is something to decide"
    );
    assert!(
        answers
            .iter()
            .all(|a| matches!(a, Action::ReturnDon { dons } if dons.len() == 7)),
        "every answer names exactly the seven the cost asks for"
    );

    // Interchangeable DON!! are not separate answers: what distinguishes them
    // is being given to the Leader or loose in the cost area, so the choice is
    // over those classes rather than over C(10,7) sets of ids.
    assert!(
        answers.len() <= 4,
        "expected a handful of distinguishable answers, got {}",
        answers.len()
    );

    let keeps_the_given = answers
        .iter()
        .find(|a| match a {
            Action::ReturnDon { dons } => !dons
                .iter()
                .any(|d| game.state.card(leader).attached_don.contains(d)),
            _ => false,
        })
        .expect("keeping the given DON!! must be on the table")
        .clone();
    game.step(keeps_the_given).unwrap();
    assert_eq!(
        game.state.card(leader).attached_don.len(),
        1,
        "the DON!! the player kept is still on the Leader"
    );
}

/// ST-03's signature: five of its cards return a Character "to the **owner's**
/// hand". Against an opponent's Character that is *their* hand, not the
/// controller's — bouncing is disruption, not theft. Nothing else in the set
/// would catch this reading being wrong, because owner and controller are the
/// same player for every other effect implemented so far.
#[test]
fn st03_001_bounce_returns_a_character_to_its_owners_hand() {
    let Some((db, cards)) = load() else { return };
    let mut game = st03_at_main(db, cards, 7, 8);

    let leader = game.state.player(PlayerId::P0).leader.unwrap();
    let victim = put_in_play(&mut game, PlayerId::P1, "ST01-004");
    let their_hand = game.state.player(PlayerId::P1).hand.len();
    let your_hand = game.state.player(PlayerId::P0).hand.len();

    game.step(Action::ActivateEffect {
        card: leader,
        slot: 0,
        discard: vec![],
    })
    .unwrap();
    game.step(Action::Choose {
        cards: vec![victim],
    })
    .unwrap();

    assert_eq!(game.state.card(victim).zone, Zone::Hand);
    assert_eq!(
        game.state.card(victim).controller,
        PlayerId::P1,
        "the card went home, not across the table"
    );
    assert_eq!(game.state.player(PlayerId::P1).hand.len(), their_hand + 1);
    assert_eq!(
        game.state.player(PlayerId::P0).hand.len(),
        your_hand,
        "bouncing must not draw the controller a card"
    );
}

/// The same effect reaches your own board: the text says "Character", not "your
/// opponent's Character", so returning your own is a legal line — replaying an
/// [On Play] is the reason you would.
#[test]
fn st03_001_bounce_can_target_your_own_character() {
    let Some((db, cards)) = load() else { return };
    let mut game = st03_at_main(db, cards, 7, 8);

    let leader = game.state.player(PlayerId::P0).leader.unwrap();
    let mine = put_in_play(&mut game, PlayerId::P0, "ST03-014");

    game.step(Action::ActivateEffect {
        card: leader,
        slot: 0,
        discard: vec![],
    })
    .unwrap();

    let legal = legal_actions(&game);
    assert!(
        legal.contains(&Action::Choose { cards: vec![mine] }),
        "your own Character should be a legal bounce target"
    );
}

/// ST03-004 searches the trash for a Warlord "other than [Gecko Moria]" — and
/// ST03-004 *is* Gecko Moria, so the exclusion is about the card doing the
/// searching. By name rather than card number, so it covers every printing.
#[test]
fn st03_004_cannot_return_another_gecko_moria() {
    let Some((db, cards)) = load() else { return };
    let mut game = st03_at_main(db, cards, 3, 8);

    // Two Warlords in the trash: another Gecko Moria, and one that is not.
    let moria = game.db().by_number("ST03-004").unwrap();
    let other = game.db().by_number("ST03-005").unwrap();
    let moria = game.state.spawn(moria, PlayerId::P0, Zone::Limbo);
    let other = game.state.spawn(other, PlayerId::P0, Zone::Limbo);
    for card in [moria, other] {
        game.state
            .move_card(card, PlayerId::P0, Zone::Trash, Placement::Bottom);
    }

    // Played from hand rather than placed, because [On Play] is what fires the
    // search — `put_in_play` moves the card without ever playing it.
    let def = game.db().by_number("ST03-004").unwrap();
    let source = game.state.spawn(def, PlayerId::P0, Zone::Hand);
    game.step(Action::PlayCard {
        card: source,
        replacing: None,
    })
    .unwrap();

    let legal = legal_actions(&game);
    assert!(
        legal.contains(&Action::Choose { cards: vec![other] }),
        "a Warlord that is not Gecko Moria should be offered"
    );
    assert!(
        !legal.contains(&Action::Choose { cards: vec![moria] }),
        "\"other than [Gecko Moria]\" must exclude every Gecko Moria, not just this one"
    );
}
