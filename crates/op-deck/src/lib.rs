//! Deck construction: import, resolution, legality, engine support, storage.
//!
//! A deck passes through four independent questions, and the whole point of
//! this crate is that they stay independent — a deck can be syntactically
//! valid, rules legal, and still not playable on this engine, and a user
//! deserves to be told which of those they are looking at:
//!
//! 1. [`text`] — is it a decklist? (format only, no card database)
//! 2. [`resolve`] — do these card numbers name cards we have?
//! 3. [`legality`] — is it a legal deck? (5-1-2, the Comprehensive Rules)
//! 4. [`compat`] — can this engine play it? (which cards have working scripts)
//!
//! Only the third is about the rules, and only the fourth is about *us*.
//! Collapsing them — refusing to save a legal deck because a card is
//! unscripted, or calling an off-colour deck "unsupported" — is the failure
//! mode this layering exists to prevent.
//!
//! [`store`] persists what survives, so a deck outlives the session that built
//! it.

pub mod compat;
pub mod legality;
pub mod resolve;
pub mod store;
pub mod text;

use serde::{Deserialize, Serialize};

/// Some number of copies of one card number.
///
/// The unit every layer here passes around, and the unit a deck is *stored* as:
/// a saved deck holds card numbers and counts, never a copy of the card's
/// printed data, which belongs to the [`op_core::card::CardDb`] and would only
/// go stale here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeckEntry {
    pub number: String,
    /// Wider than the 4 the rules allow, so that an over-count is a legality
    /// error with a number in it rather than a silently saturated 4.
    pub quantity: u32,
}

impl DeckEntry {
    pub fn new(number: impl Into<String>, quantity: u32) -> DeckEntry {
        DeckEntry {
            number: number.into(),
            quantity,
        }
    }
}

/// Expands `(number, quantity)` entries into the flat card list the engine
/// wants.
///
/// The order is load-bearing and this is the only place that knows it: setup
/// assigns instance ids by walking the list, so regrouping it produces
/// different ids and a different game from the same seed. Entries expand in
/// order, each run of copies together.
pub fn expand(leader: &str, entries: &[DeckEntry]) -> op_core::DeckList {
    let mut cards = Vec::new();
    for entry in entries {
        for _ in 0..entry.quantity {
            cards.push(entry.number.clone());
        }
    }
    op_core::DeckList {
        leader: leader.to_string(),
        cards,
    }
}

/// Groups a flat card list back into counted entries, first mention first.
///
/// The inverse of [`expand`] for any list whose copies are contiguous, which is
/// how every list this project produces is built. A list that interleaves
/// copies collapses to the same entries but would not expand back to the same
/// order — so this is for reading a decklist into an editor, not for
/// round-tripping one through a game.
pub fn collapse(cards: &[String]) -> Vec<DeckEntry> {
    let mut entries: Vec<DeckEntry> = Vec::new();
    for number in cards {
        match entries.iter_mut().find(|e| &e.number == number) {
            Some(entry) => entry.quantity += 1,
            None => entries.push(DeckEntry::new(number.clone(), 1)),
        }
    }
    entries
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use op_core::card::{CardDb, CardDef, Category, Color};

    /// A card pool for this crate's tests.
    ///
    /// Synthetic rather than the fetched `data/`: every test states the
    /// characteristics it depends on, and the suite runs on a bare clone, where
    /// there is no card data at all.
    pub(crate) fn test_db() -> CardDb {
        let mut db = CardDb::empty();
        db.insert(leader("RED-LDR", &[Color::Red]));
        db.insert(leader("BLUE-LDR", &[Color::Blue]));

        for number in [
            "RED-002", "RED-003", "RED-006", "RED-007", "RED-008", "RED-009", "RED-010", "RED-011",
            "RED-012", "RED-013",
        ] {
            db.insert(card(number, Category::Character, &[Color::Red]));
        }
        db.insert(card("RED-EVENT", Category::Event, &[Color::Red]));
        db.insert(card("RED-STAGE", Category::Stage, &[Color::Red]));
        // Two-colour, for 5-1-2-2's "a colour included on the Leader".
        db.insert(card(
            "RG-005",
            Category::Character,
            &[Color::Red, Color::Green],
        ));
        db.insert(card("BLUE-002", Category::Character, &[Color::Blue]));
        db.insert(card("GREEN-002", Category::Character, &[Color::Green]));
        db
    }

    fn leader(number: &str, colors: &[Color]) -> CardDef {
        CardDef {
            life: Some(5),
            power: Some(5000),
            ..card(number, Category::Leader, colors)
        }
    }

    fn card(number: &str, category: Category, colors: &[Color]) -> CardDef {
        CardDef {
            number: number.to_string(),
            name: number.to_string(),
            category,
            colors: colors.to_vec(),
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

    #[test]
    fn expanding_repeats_each_entry_in_place() {
        let list = expand(
            "ST01-001",
            &[DeckEntry::new("ST01-002", 3), DeckEntry::new("ST01-003", 1)],
        );
        assert_eq!(list.leader, "ST01-001");
        assert_eq!(list.cards, ["ST01-002", "ST01-002", "ST01-002", "ST01-003"]);
    }

    #[test]
    fn collapsing_a_list_of_runs_inverts_expanding_it() {
        let entries = vec![DeckEntry::new("ST01-002", 3), DeckEntry::new("ST01-003", 1)];
        assert_eq!(collapse(&expand("ST01-001", &entries).cards), entries);
    }

    /// Interleaved copies still count correctly; only the order they expand
    /// back to differs, which is why `collapse` is for editing rather than for
    /// round-tripping a deck through a game.
    #[test]
    fn collapsing_counts_interleaved_copies_at_their_first_mention() {
        let cards = ["A-1", "B-1", "A-1"].map(String::from);
        assert_eq!(
            collapse(&cards),
            [DeckEntry::new("A-1", 2), DeckEntry::new("B-1", 1)]
        );
    }
}
