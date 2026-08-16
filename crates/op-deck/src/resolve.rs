//! Card resolution: turning card numbers into cards.
//!
//! The step between "this is a decklist" and "this is a legal deck". It answers
//! one question — does the loaded [`CardDb`] contain this number? — and nothing
//! else. In particular it does not decide whether a deck is legal, because a
//! deck whose cards are all unrecognised is not an *illegal* deck, it is a deck
//! we cannot see. Card data is fetched per pack, so on most installs some of
//! both will be true at once.

use op_core::card::{CardDb, Category};
use op_core::ids::CardDefId;

use crate::DeckEntry;

/// A decklist entry that named a card we have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCard {
    pub number: String,
    pub quantity: u32,
    pub def: CardDefId,
}

/// A decklist resolved against the card database.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedDeck {
    /// Every entry whose card is a Leader. More than one is a legality error,
    /// not a resolution error, so all of them survive to be reported.
    pub leaders: Vec<ResolvedCard>,
    /// Everything else, in decklist order.
    pub cards: Vec<ResolvedCard>,
    /// Entries naming no card in the database — a typo, or a pack that has not
    /// been fetched. Kept in decklist order.
    pub unknown: Vec<DeckEntry>,
}

impl ResolvedDeck {
    /// The Leader, when the list names exactly one.
    pub fn leader(&self) -> Option<&ResolvedCard> {
        match self.leaders.as_slice() {
            [only] => Some(only),
            _ => None,
        }
    }

    /// Copies of non-Leader cards the list asks for, recognised or not.
    ///
    /// Unknown entries count. A 50-card list with three unfetched cards is the
    /// right size and missing three cards, and reporting it as a 47-card deck
    /// would send the user hunting for an error they did not make.
    pub fn deck_size(&self) -> u32 {
        let known: u32 = self.cards.iter().map(|c| c.quantity).sum();
        let unknown: u32 = self.unknown.iter().map(|e| e.quantity).sum();
        known + unknown
    }

    /// The deck as the engine wants it, or `None` without exactly one Leader.
    ///
    /// Unknown cards are expanded too: `Game::new` resolves numbers itself and
    /// reports the ones it cannot find, and silently dropping them here would
    /// hand it a short deck with no explanation.
    pub fn to_decklist(&self) -> Option<op_core::DeckList> {
        let leader = self.leader()?;
        let mut entries: Vec<DeckEntry> = self
            .cards
            .iter()
            .map(|c| DeckEntry::new(c.number.clone(), c.quantity))
            .collect();
        entries.extend(self.unknown.iter().cloned());
        Some(crate::expand(&leader.number, &entries))
    }
}

/// Resolves decklist entries against `db`, splitting the Leader out.
pub fn resolve(entries: &[DeckEntry], db: &CardDb) -> ResolvedDeck {
    let mut out = ResolvedDeck::default();

    for entry in entries {
        let Some(def) = db.by_number(&entry.number) else {
            out.unknown.push(entry.clone());
            continue;
        };
        let card = ResolvedCard {
            number: entry.number.clone(),
            quantity: entry.quantity,
            def,
        };
        // The Leader is identified by what it *is*, not by where it sits in the
        // list: exports disagree about whether it comes first.
        if db.get(def).category == Category::Leader {
            out.leaders.push(card);
        } else {
            out.cards.push(card);
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::test_db;

    #[test]
    fn the_leader_is_found_by_category_not_by_position() {
        let db = test_db();
        let deck = resolve(
            &[DeckEntry::new("RED-002", 4), DeckEntry::new("RED-LDR", 1)],
            &db,
        );
        assert_eq!(deck.leader().unwrap().number, "RED-LDR");
        assert_eq!(deck.cards.len(), 1);
    }

    #[test]
    fn an_unfetched_or_mistyped_number_resolves_to_unknown() {
        let db = test_db();
        let deck = resolve(
            &[DeckEntry::new("RED-LDR", 1), DeckEntry::new("OP99-999", 2)],
            &db,
        );
        assert_eq!(deck.unknown, [DeckEntry::new("OP99-999", 2)]);
        assert!(deck.cards.is_empty());
    }

    /// A deck missing three unfetched cards is a 50-card deck with three cards
    /// missing, not a 47-card deck.
    #[test]
    fn unknown_cards_still_count_towards_the_deck_size() {
        let db = test_db();
        let deck = resolve(
            &[
                DeckEntry::new("RED-LDR", 1),
                DeckEntry::new("RED-002", 47),
                DeckEntry::new("OP99-999", 3),
            ],
            &db,
        );
        assert_eq!(deck.deck_size(), 50);
    }

    #[test]
    fn two_leaders_both_survive_for_legality_to_report() {
        let db = test_db();
        let deck = resolve(
            &[DeckEntry::new("RED-LDR", 1), DeckEntry::new("BLUE-LDR", 1)],
            &db,
        );
        assert_eq!(deck.leaders.len(), 2);
        assert!(deck.leader().is_none());
    }

    #[test]
    fn the_expanded_decklist_keeps_unknown_cards_for_the_engine_to_report() {
        let db = test_db();
        let deck = resolve(
            &[
                DeckEntry::new("RED-LDR", 1),
                DeckEntry::new("RED-002", 2),
                DeckEntry::new("OP99-999", 1),
            ],
            &db,
        );
        let list = deck.to_decklist().unwrap();
        assert_eq!(list.leader, "RED-LDR");
        assert_eq!(list.cards, ["RED-002", "RED-002", "OP99-999"]);
    }
}
