//! The deck import pipeline, end to end, against real card data.
//!
//! `op-deck`'s own tests run on a synthetic pool so they work on a bare clone.
//! These are the other half: the built-in decks, the fetched database, and the
//! real scripts, checked through every stage a pasted decklist passes. They
//! skip themselves when `data/` is absent, so a bare clone stays green — which
//! also means running them proves something only after an ingest.

mod common;

use op_cards::{decks, Cards};
use op_deck::{collapse, compat, legality, resolve, text};

/// Export, reimport, and land on exactly the deck we started with.
///
/// Byte equality of the expanded `DeckList`, not merely the same cards: setup
/// assigns instance ids by walking that list, so a deck that came back
/// regrouped would play a different game from the same seed and every session
/// log recorded against it would stop reproducing.
#[test]
fn every_builtin_deck_survives_an_export_and_reimport() {
    let Some(db) = common::card_db() else { return };

    for deck in decks::ALL {
        let original = deck.list();
        let exported = text::write(&original.leader, &collapse(&original.cards));

        let parsed = text::parse(&exported);
        assert_eq!(parsed.problems, [], "{} did not re-parse", deck.id);

        let resolved = resolve::resolve(&parsed.entries, &db);
        assert_eq!(resolved.unknown, [], "{} has unresolved cards", deck.id);
        assert_eq!(
            resolved.to_decklist().as_ref(),
            Some(&original),
            "{} did not round-trip",
            deck.id
        );
    }
}

/// 5-1-2, including the colour rule the engine's own `validate_deck` does not
/// check. `op_cards::decks` tests size and the copy limit; this is the first
/// thing to hold the built-in lists to 5-1-2-2 and 5-1-2-1.
#[test]
fn every_builtin_deck_is_legal_under_the_full_construction_rules() {
    let Some(db) = common::card_db() else { return };

    for deck in decks::ALL {
        let list = deck.list();
        // The Leader is a separate field on `DeckList`; the colour rule needs
        // it back in the entry list to have anything to check against.
        let mut entries = vec![op_deck::DeckEntry::new(list.leader.clone(), 1)];
        entries.extend(collapse(&list.cards));
        let resolved = resolve::resolve(&entries, &db);

        let errors = legality::check(&resolved, &db);
        assert_eq!(
            errors.iter().map(|e| e.to_string()).collect::<Vec<_>>(),
            Vec::<String>::new(),
            "{} is not a legal deck",
            deck.id
        );
    }
}

/// A deck the menu offers must be a deck the engine can actually play. This is
/// the gate `op_cards::decks`' "every scripted set is offered" test cannot
/// reach: a set can be listed, and still contain one card whose text nothing
/// implements.
#[test]
fn every_builtin_deck_is_fully_supported_by_this_build() {
    let Some(db) = common::card_db() else { return };
    let scripts = Cards::new(&db);

    for deck in decks::ALL {
        let list = deck.list();
        let mut entries = vec![op_deck::DeckEntry::new(list.leader.clone(), 1)];
        entries.extend(collapse(&list.cards));
        let resolved = resolve::resolve(&entries, &db);

        let report = compat::check(&resolved, &db, &scripts);
        let problems: Vec<String> = report
            .problems()
            .map(|c| format!("{} — {}", c.number, c.support.reason().unwrap_or("unknown")))
            .collect();
        assert_eq!(
            problems,
            Vec::<String>::new(),
            "{} contains cards this build cannot play",
            deck.id
        );
    }
}

/// The pasted-decklist path, with the mistakes a user actually makes: a deck
/// name pasted along with the list, a pack they have not fetched, and a fifth
/// copy. Each belongs to a different layer, and each is reported by that layer
/// rather than collapsed into one "invalid deck".
#[test]
fn a_decklist_with_mistakes_reports_each_one_separately() {
    let Some(db) = common::card_db() else { return };

    let pasted = "\
Purple Luffy v3
1 ST01-001
5 ST01-002
4 OP99-999
";
    let parsed = text::parse(pasted);
    let resolved = resolve::resolve(&parsed.entries, &db);
    let errors = legality::check(&resolved, &db);

    // The stray title is a format problem...
    assert_eq!(parsed.problems.len(), 1, "{:?}", parsed.problems);
    assert_eq!(parsed.problems[0].line, 1);
    // ...the unfetched card is a resolution problem...
    assert_eq!(resolved.unknown.len(), 1);
    assert_eq!(resolved.unknown[0].number, "OP99-999");
    // ...and the fifth copy is a rules problem. Three layers, three reports.
    assert!(errors.contains(&legality::LegalityError::TooManyCopies {
        number: "ST01-002".into(),
        count: 5,
    }));
    assert!(errors.contains(&legality::LegalityError::DeckSize { found: 9 }));
}

/// The kernel and the deck builder must agree about 5-1-2.
///
/// They are two implementations because they answer different shapes of the
/// question — `Game::new` wants the first violation, a deck builder wants all
/// of them — and two implementations of one rule set drift. A deck the builder
/// calls legal must start, and a deck it rejects must not.
#[test]
fn the_kernel_and_the_deck_builder_agree_about_legality() {
    let Some(db) = common::card_db() else { return };
    let db = std::sync::Arc::new(db);
    let scripts: std::sync::Arc<dyn op_core::script::ScriptSource + Send + Sync> =
        std::sync::Arc::new(Cards::new(&db));

    let base = decks::st01();
    let mut cases: Vec<(&str, op_core::DeckList)> = vec![("the deck as printed", base.clone())];

    // One case per clause, each breaking exactly one.
    let mut short = base.clone();
    short.cards.pop();
    cases.push(("49 cards (5-1-2)", short));

    let mut five = base.clone();
    five.cards[0] = five.cards[1].clone();
    five.cards[2] = five.cards[1].clone();
    five.cards[3] = five.cards[1].clone();
    five.cards[4] = five.cards[1].clone();
    cases.push(("five copies (5-1-2-3)", five));

    // ST-01 is red; ST-03's Leader is not, so any ST-03 Character is off-colour.
    let mut off_colour = base.clone();
    off_colour.cards[0] = "ST03-002".to_string();
    cases.push(("an off-colour card (5-1-2-2)", off_colour));

    let mut leader_inside = base.clone();
    leader_inside.cards[0] = "ST02-001".to_string();
    cases.push(("a Leader in the fifty (5-1-2-1)", leader_inside));

    for (what, list) in cases {
        let mut entries = vec![op_deck::DeckEntry::new(list.leader.clone(), 1)];
        entries.extend(collapse(&list.cards));
        let builder_errors = legality::check(&resolve::resolve(&entries, &db), &db);

        let config = op_core::GameConfig {
            seed: 1,
            first_player: op_core::PlayerId::P0,
            decks: [list.clone(), decks::st02()],
            allow_illegal_decks: false,
        };
        let kernel = op_core::Game::new(config, db.clone(), scripts.clone());

        assert_eq!(
            builder_errors.is_empty(),
            kernel.is_ok(),
            "{what}: builder said {builder_errors:?}, kernel said {:?}",
            kernel.err()
        );
    }
}

/// The boundary between "not a decklist line" and "not a card": a number that
/// is *shaped* like a card number is the resolver's problem, not the parser's.
///
/// It matters because the two need different advice — "check the spelling"
/// against "fetch that pack" — and a parser that knew the card universe could
/// not tell a typo from an unfetched set.
#[test]
fn a_plausible_but_nonexistent_number_is_unknown_rather_than_unparseable() {
    let Some(db) = common::card_db() else { return };

    // A letter O for a zero — the typo this distinction exists for.
    let parsed = text::parse("4 STO1-002");
    assert_eq!(parsed.problems, []);

    let resolved = resolve::resolve(&parsed.entries, &db);
    assert_eq!(resolved.unknown.len(), 1);
    assert_eq!(resolved.unknown[0].number, "STO1-002");
}
