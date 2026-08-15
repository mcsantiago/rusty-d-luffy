//! Deck construction rules — 5-1-2, checked at setup.
//!
//! A self-contained pool rather than the shared fixture: every card there is
//! Red, and two of these four clauses are about colour and category.

use std::sync::Arc;

use op_core::card::{CardDb, CardDef, Category, Color};
use op_core::{DeckList, Game, GameConfig, NoScripts, PlayerId, SetupError};

fn card(number: &str, category: Category, colors: &[Color]) -> CardDef {
    CardDef {
        number: number.to_string(),
        name: number.to_string(),
        category,
        colors: colors.to_vec(),
        cost: 1,
        life: matches!(category, Category::Leader).then_some(5),
        power: Some(1000),
        counter: Some(1000),
        types: Vec::new(),
        attributes: Vec::new(),
        keywords: Vec::new(),
        effect: None,
        trigger: None,
    }
}

fn db() -> CardDb {
    let mut db = CardDb::empty();
    db.insert(card("RED-LDR", Category::Leader, &[Color::Red]));
    db.insert(card("RED-CHR", Category::Character, &[Color::Red]));
    db.insert(card("RED-EVT", Category::Event, &[Color::Red]));
    db.insert(card("BLUE-CHR", Category::Character, &[Color::Blue]));
    // Two-coloured, for "a colour *included on* the Leader" rather than "the
    // Leader's colour".
    db.insert(card(
        "RG-CHR",
        Category::Character,
        &[Color::Red, Color::Green],
    ));
    db
}

/// 25 Characters and 25 Events, all Red — legal under `RED-LDR`.
fn legal_deck() -> DeckList {
    let mut cards = vec!["RED-CHR".to_string(); 25];
    cards.extend(vec!["RED-EVT".to_string(); 25]);
    DeckList {
        leader: "RED-LDR".to_string(),
        cards,
    }
}

/// Setup with `deck` opposite a legal one. The copy limit is waived for these
/// decks by construction — 25 of a card is well over 4 — so every test here
/// sets `allow_illegal_decks: false` and reads the error it expects.
fn setup(deck: DeckList) -> Result<(), SetupError> {
    let config = GameConfig {
        seed: 1,
        first_player: PlayerId::P0,
        decks: [deck, legal_deck()],
        allow_illegal_decks: false,
    };
    Game::new(config, Arc::new(db()), Arc::new(NoScripts::default())).map(|_| ())
}

/// The baseline every other case is a deviation from: 50 on-colour deck cards,
/// refused for the copy limit and nothing else.
///
/// Worth pinning, because it is what makes the rest of this file mean what it
/// says. A test asserting `OffColor` proves nothing if the deck would have been
/// rejected anyway for a reason nobody checked.
#[test]
fn the_baseline_deck_is_refused_only_for_the_copy_limit() {
    assert_eq!(
        setup(legal_deck()),
        Err(SetupError::TooManyCopies("RED-CHR".to_string()))
    );
}

#[test]
fn rule_5_1_2_a_deck_must_be_fifty_cards() {
    let mut deck = legal_deck();
    deck.cards.pop();
    assert_eq!(setup(deck), Err(SetupError::DeckSize(49)));
}

/// 5-1-2-2: "Only cards of a color included on the Leader card can be included
/// in a deck."
#[test]
fn rule_5_1_2_2_a_card_sharing_no_colour_with_the_leader_is_refused() {
    let mut deck = legal_deck();
    // Four copies, so the copy limit cannot be what rejects it.
    deck.cards = vec!["BLUE-CHR".to_string(); 4];
    deck.cards.extend(vec!["RED-CHR".to_string(); 4]);
    deck.cards.extend(vec!["RED-EVT".to_string(); 42]);
    assert_eq!(
        setup(deck),
        Err(SetupError::OffColor {
            number: "BLUE-CHR".to_string(),
            leader: "RED-LDR".to_string(),
        })
    );
}

/// 5-1-2-2 asks for a shared colour, not an identical one.
#[test]
fn rule_5_1_2_2_one_shared_colour_is_enough() {
    let mut deck = legal_deck();
    deck.cards = vec!["RG-CHR".to_string(); 4];
    deck.cards.extend(vec!["RED-CHR".to_string(); 4]);
    deck.cards.extend(vec!["RED-EVT".to_string(); 42]);
    // Rejected for the copy limit on RED-EVT, never for RG-CHR's colour.
    assert_eq!(
        setup(deck),
        Err(SetupError::TooManyCopies("RED-EVT".to_string()))
    );
}

/// 5-1-2-1: "A deck is a bundle of cards made up of Character cards, Event
/// cards, and Stage cards." A Leader among the fifty is not one of those.
#[test]
fn rule_5_1_2_1_a_leader_cannot_be_in_the_deck() {
    let mut deck = legal_deck();
    deck.cards = vec!["RED-LDR".to_string(); 4];
    deck.cards.extend(vec!["RED-CHR".to_string(); 4]);
    deck.cards.extend(vec!["RED-EVT".to_string(); 42]);
    assert_eq!(
        setup(deck),
        Err(SetupError::NotADeckCard("RED-LDR".to_string()))
    );
}

/// Waiving validation has to waive all of it: kernel tests build three-card
/// decks of whatever they need, and a colour rule that still applied would
/// break every one of them.
#[test]
fn allow_illegal_decks_waives_the_colour_and_category_rules_too() {
    let config = GameConfig {
        seed: 1,
        first_player: PlayerId::P0,
        decks: [
            DeckList {
                leader: "RED-LDR".to_string(),
                cards: vec!["BLUE-CHR".to_string(); 3],
            },
            DeckList {
                leader: "RED-LDR".to_string(),
                cards: vec!["BLUE-CHR".to_string(); 3],
            },
        ],
        allow_illegal_decks: true,
    };
    assert!(Game::new(config, Arc::new(db()), Arc::new(NoScripts::default())).is_ok());
}
