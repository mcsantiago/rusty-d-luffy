//! Test fixtures.
//!
//! Each integration test binary compiles this module separately and uses a
//! different subset of it, so anything used by only one of them looks dead to
//! the others.
#![allow(dead_code)]
//!
//!
//! Kernel tests build their own tiny card pool rather than loading `data/`, so
//! the suite runs without the ingest step and each test states exactly the
//! characteristics it depends on.

use std::sync::Arc;

use op_core::card::{CardDb, CardDef, Category, Color, Keyword};
use op_core::script::{CardScript, ScriptSource};
use op_core::{CardDefId, DeckList, Game, GameConfig, PlayerId};

pub struct TestCards {
    pub db: CardDb,
}

impl TestCards {
    pub fn new() -> TestCards {
        let mut db = CardDb::empty();

        db.insert(leader("LDR-001", 5, 5000));
        db.insert(leader("LDR-002", 4, 5000));

        // A plain vanilla body with a Counter value.
        db.insert(character("CHR-2K", 1, 2000, Some(1000), &[]));
        db.insert(character("CHR-5K", 3, 5000, Some(1000), &[]));
        db.insert(character("CHR-7K", 5, 7000, None, &[]));
        db.insert(character("CHR-BLOCK", 1, 1000, None, &[Keyword::Blocker]));
        db.insert(character("CHR-RUSH", 2, 4000, None, &[Keyword::Rush]));
        db.insert(character(
            "CHR-DOUBLE",
            4,
            6000,
            None,
            &[Keyword::DoubleAttack],
        ));
        db.insert(character("CHR-BANISH", 4, 6000, None, &[Keyword::Banish]));
        db.insert(character(
            "CHR-UNBLOCK",
            3,
            5000,
            None,
            &[Keyword::Unblockable],
        ));

        // A card with a [Trigger], for damage-step suspension tests.
        let mut trig = character("CHR-TRIGGER", 1, 1000, Some(1000), &[]);
        trig.trigger = Some("[Trigger] Draw 1 card.".to_string());
        db.insert(trig);

        db.insert(event("EVT-1"));

        TestCards { db }
    }

    pub fn def(&self, number: &str) -> CardDefId {
        self.db
            .by_number(number)
            .unwrap_or_else(|| panic!("test card {number} not defined"))
    }
}

fn leader(number: &str, life: u8, power: i32) -> CardDef {
    CardDef {
        number: number.to_string(),
        name: number.to_string(),
        category: Category::Leader,
        colors: vec![Color::Red],
        cost: 0,
        life: Some(life),
        power: Some(power),
        counter: None,
        types: vec!["Test".to_string()],
        attributes: Vec::new(),
        keywords: Vec::new(),
        effect: None,
        trigger: None,
    }
}

fn character(
    number: &str,
    cost: u8,
    power: i32,
    counter: Option<i32>,
    keywords: &[Keyword],
) -> CardDef {
    CardDef {
        number: number.to_string(),
        name: number.to_string(),
        category: Category::Character,
        colors: vec![Color::Red],
        cost,
        life: None,
        power: Some(power),
        counter,
        types: vec!["Test".to_string()],
        attributes: Vec::new(),
        keywords: keywords.to_vec(),
        effect: None,
        trigger: None,
    }
}

/// An Event, which is a card with no power and no counter (2-7-1). Cost 1 so a
/// test at its first Main Phase can afford it and still have DON!! left over.
fn event(number: &str) -> CardDef {
    CardDef {
        number: number.to_string(),
        name: number.to_string(),
        category: Category::Event,
        colors: vec![Color::Red],
        cost: 1,
        life: None,
        power: None,
        counter: None,
        types: vec!["Test".to_string()],
        attributes: Vec::new(),
        keywords: Vec::new(),
        effect: None,
        trigger: None,
    }
}

/// Scripts for the test pool. Every test card is vanilla unless a test
/// registers otherwise.
#[derive(Default)]
pub struct TestScripts {
    empty: CardScript,
    scripts: Vec<(CardDefId, CardScript)>,
}

impl TestScripts {
    pub fn with(mut self, def: CardDefId, script: CardScript) -> TestScripts {
        self.scripts.push((def, script));
        self
    }
}

impl ScriptSource for TestScripts {
    fn script(&self, def: CardDefId) -> &CardScript {
        self.scripts
            .iter()
            .find(|(d, _)| *d == def)
            .map(|(_, s)| s)
            .unwrap_or(&self.empty)
    }
}

/// Builds a game from a repeated-card decklist. Deck legality is waived so
/// tests can stack a deck with exactly what they need.
pub fn game_with(
    cards: &TestCards,
    scripts: TestScripts,
    seed: u64,
    p0: (&str, Vec<&str>),
    p1: (&str, Vec<&str>),
) -> (Game, op_core::StepOutcome) {
    let config = GameConfig {
        seed,
        first_player: PlayerId::P0,
        decks: [decklist(p0), decklist(p1)],
        allow_illegal_decks: true,
    };
    Game::new(config, Arc::new(cards.db.clone()), Arc::new(scripts))
        .expect("test game setup should succeed")
}

fn decklist((leader, cards): (&str, Vec<&str>)) -> DeckList {
    DeckList {
        leader: leader.to_string(),
        cards: cards.into_iter().map(|c| c.to_string()).collect(),
    }
}

/// A deck of `n` copies of `number`.
pub fn deck_of(number: &str, n: usize) -> Vec<&str> {
    std::iter::repeat_n(number, n).collect()
}
