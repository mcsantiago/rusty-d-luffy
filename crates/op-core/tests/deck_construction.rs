//! #15: the Leader colour restriction (5-1-2-2).
//!
//! `Game::new` already enforced 5-1-2 (card count) and 5-1-2-3 (4-copy limit)
//! through `validate_deck`, gated by `GameConfig::allow_illegal_decks`; this
//! is the third leg of deck construction, and had no test coverage — or
//! enforcement — until #15.
//!
//! Self-contained rather than built on `tests/common`, so these run without
//! `data/` and without perturbing the shared Red-only test pool every other
//! suite reads from.

use std::sync::Arc;

use op_core::card::{CardDb, CardDef, Category, Color};
use op_core::{DeckList, Game, GameConfig, NoScripts, PlayerId, SetupError};

fn leader(number: &str, colors: Vec<Color>) -> CardDef {
    CardDef {
        number: number.to_string(),
        name: number.to_string(),
        category: Category::Leader,
        colors,
        cost: 0,
        life: Some(5),
        power: Some(5000),
        counter: None,
        types: Vec::new(),
        attributes: Vec::new(),
        keywords: Vec::new(),
        effect: None,
        trigger: None,
    }
}

fn character(number: &str, colors: Vec<Color>) -> CardDef {
    CardDef {
        number: number.to_string(),
        name: number.to_string(),
        category: Category::Character,
        colors,
        cost: 1,
        life: None,
        power: Some(1000),
        counter: Some(1000),
        types: Vec::new(),
        attributes: Vec::new(),
        keywords: Vec::new(),
        effect: None,
        trigger: None,
    }
}

/// A Leader plus 50 distinct, single-copy Character numbers, all sharing the
/// Leader's colour, so the deck is legal aside from whatever a test
/// perturbs afterwards. One copy each sidesteps 5-1-2-3 entirely — this file
/// only exercises the colour check.
fn legal_pool(
    leader_number: &str,
    leader_colors: Vec<Color>,
    filler_color: Color,
) -> (CardDb, DeckList) {
    let mut db = CardDb::empty();
    db.insert(leader(leader_number, leader_colors));
    let mut cards = Vec::new();
    for i in 0..50 {
        let number = format!("CHR-{i:03}");
        db.insert(character(&number, vec![filler_color]));
        cards.push(number);
    }
    let deck = DeckList {
        leader: leader_number.to_string(),
        cards,
    };
    (db, deck)
}

#[test]
fn a_deck_that_matches_the_leaders_colour_is_accepted() {
    let (db, deck) = legal_pool("LDR-RED", vec![Color::Red], Color::Red);
    let opponent = deck.clone();
    let config = GameConfig {
        seed: 1,
        first_player: PlayerId::P0,
        decks: [deck, opponent],
        allow_illegal_decks: false,
    };
    Game::new(config, Arc::new(db), Arc::new(NoScripts::default()))
        .expect("a deck built entirely from the Leader's own colour must be legal");
}

#[test]
fn a_deck_with_a_card_outside_the_leaders_colours_is_rejected() {
    let (mut db, deck) = legal_pool("LDR-RED", vec![Color::Red], Color::Red);
    db.insert(character("CHR-BLUE", vec![Color::Blue]));
    let opponent = deck.clone();
    let mut deck = deck;
    deck.cards[0] = "CHR-BLUE".to_string();

    let config = GameConfig {
        seed: 1,
        first_player: PlayerId::P0,
        decks: [deck, opponent],
        allow_illegal_decks: false,
    };
    let err = Game::new(config, Arc::new(db), Arc::new(NoScripts::default()))
        .err()
        .expect("a colour mismatch must be rejected");
    assert!(
        matches!(&err, SetupError::WrongColour(n) if n == "CHR-BLUE"),
        "expected WrongColour(\"CHR-BLUE\"), got {err:?}"
    );
}

/// The escape hatch 8-1-3-4-3 needs — `Game::new` already skips
/// `validate_deck` entirely under `allow_illegal_decks`, so this pins that
/// the colour check does not somehow bypass the same gate.
#[test]
fn allow_illegal_decks_waives_the_colour_restriction() {
    let (mut db, deck) = legal_pool("LDR-RED", vec![Color::Red], Color::Red);
    db.insert(character("CHR-BLUE", vec![Color::Blue]));
    let opponent = deck.clone();
    let mut deck = deck;
    deck.cards[0] = "CHR-BLUE".to_string();

    let config = GameConfig {
        seed: 1,
        first_player: PlayerId::P0,
        decks: [deck, opponent],
        allow_illegal_decks: true,
    };
    Game::new(config, Arc::new(db), Arc::new(NoScripts::default()))
        .expect("allow_illegal_decks must waive 5-1-2-2 too");
}

/// 5-1-2-2 asks for overlap, not a matching colour set: ST-30's Leader is
/// Red/Green, and a card that is only Green (not Red) is still legal under
/// it.
#[test]
fn sharing_one_of_a_multi_colour_leaders_colours_is_enough() {
    let (db, deck) = legal_pool("LDR-RG", vec![Color::Red, Color::Green], Color::Green);
    let opponent = deck.clone();
    let config = GameConfig {
        seed: 1,
        first_player: PlayerId::P0,
        decks: [deck, opponent],
        allow_illegal_decks: false,
    };
    Game::new(config, Arc::new(db), Arc::new(NoScripts::default()))
        .expect("a Green-only card must be legal under a Red/Green Leader");
}
