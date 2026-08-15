//! Engine compatibility: which cards in a deck this build can actually play.
//!
//! Deliberately not a rules question. A deck can be perfectly legal and still
//! contain a card whose text nothing implements, and the honest thing to tell a
//! user is exactly that — rather than calling the deck invalid, which it is
//! not, or saying nothing, which is how a card silently plays as a vanilla body
//! and quietly loses the game.
//!
//! This module owns the *vocabulary* only. What each card is worth is
//! [`CardSupport`], implemented by whoever holds the scripts — `op-cards` — so
//! that adding a set changes one crate and this one keeps working.

use op_core::card::{CardDb, CardDef};
use op_core::ids::CardDefId;

use crate::resolve::ResolvedDeck;

/// How completely the engine implements one card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Support {
    /// Plays as printed: either it has a script, or it has no text needing one.
    Full,
    /// Scripted, but the script is known not to do everything the card says.
    Partial(String),
    /// Has printed text and no script. It will play, as a vanilla body — which
    /// is precisely the failure worth warning about.
    Unsupported(String),
    /// Not in the card database, so there is nothing to judge. Distinct from
    /// unsupported: fetching the pack may well resolve it.
    Unknown,
}

impl Support {
    /// Whether the card does everything it says it does.
    pub fn is_full(&self) -> bool {
        matches!(self, Support::Full)
    }

    /// Why the card is not fully supported, where there is a reason to give.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Support::Partial(why) | Support::Unsupported(why) => Some(why),
            Support::Full | Support::Unknown => None,
        }
    }
}

/// Whoever knows what this build implements.
///
/// A trait rather than a direct call so that deck handling does not depend on
/// the card scripts: `op-cards` implements it, and this crate stays testable
/// against a stub with no card data at all.
pub trait CardSupport {
    /// `def` and `card` are the same card — the id for a script lookup, the
    /// printed face for deciding whether a script is even needed.
    fn support(&self, def: CardDefId, card: &CardDef) -> Support;
}

/// One card's standing, with the number of copies riding on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardCompatibility {
    pub number: String,
    pub quantity: u32,
    pub support: Support,
}

/// Copies at each level of support. Counted in copies rather than distinct
/// cards because that is what a deck is: 4 unsupported copies of one card is a
/// bigger hole than 1 copy each of two.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SupportSummary {
    pub full: u32,
    pub partial: u32,
    pub unsupported: u32,
    pub unknown: u32,
}

impl SupportSummary {
    pub fn total(&self) -> u32 {
        self.full + self.partial + self.unsupported + self.unknown
    }

    /// Whether every copy in the deck plays as printed.
    pub fn is_complete(&self) -> bool {
        self.total() == self.full
    }
}

/// A deck's compatibility, card by card.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeckCompatibility {
    /// The Leader first where there is one, then deck cards in decklist order.
    pub cards: Vec<CardCompatibility>,
}

impl DeckCompatibility {
    pub fn summary(&self) -> SupportSummary {
        let mut out = SupportSummary::default();
        for card in &self.cards {
            let bucket = match card.support {
                Support::Full => &mut out.full,
                Support::Partial(_) => &mut out.partial,
                Support::Unsupported(_) => &mut out.unsupported,
                Support::Unknown => &mut out.unknown,
            };
            *bucket += card.quantity;
        }
        out
    }

    /// Every card that does not play as printed, in decklist order.
    pub fn problems(&self) -> impl Iterator<Item = &CardCompatibility> {
        self.cards.iter().filter(|c| !c.support.is_full())
    }
}

/// Assesses every card in `deck`, Leader included.
///
/// The Leader counts: its text is as likely to be unimplemented as any
/// Character's, and it is in play from turn one.
pub fn check(deck: &ResolvedDeck, db: &CardDb, support: &dyn CardSupport) -> DeckCompatibility {
    let mut cards = Vec::new();

    for card in deck.leaders.iter().chain(deck.cards.iter()) {
        cards.push(CardCompatibility {
            number: card.number.clone(),
            quantity: card.quantity,
            support: support.support(card.def, db.get(card.def)),
        });
    }
    for entry in &deck.unknown {
        cards.push(CardCompatibility {
            number: entry.number.clone(),
            quantity: entry.quantity,
            support: Support::Unknown,
        });
    }

    DeckCompatibility { cards }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::resolve;
    use crate::tests::test_db;
    use crate::DeckEntry;

    /// Everything is supported except the cards named.
    struct Stub(&'static [&'static str]);

    impl CardSupport for Stub {
        fn support(&self, _def: CardDefId, card: &CardDef) -> Support {
            if self.0.contains(&card.number.as_str()) {
                Support::Unsupported("card script unavailable".into())
            } else {
                Support::Full
            }
        }
    }

    fn compat(entries: &[DeckEntry], unsupported: &'static [&'static str]) -> DeckCompatibility {
        let db = test_db();
        check(&resolve(entries, &db), &db, &Stub(unsupported))
    }

    #[test]
    fn the_leader_is_assessed_and_comes_first() {
        let result = compat(
            &[DeckEntry::new("RED-002", 4), DeckEntry::new("RED-LDR", 1)],
            &[],
        );
        assert_eq!(result.cards[0].number, "RED-LDR");
    }

    #[test]
    fn support_is_counted_in_copies_not_distinct_cards() {
        let result = compat(
            &[
                DeckEntry::new("RED-LDR", 1),
                DeckEntry::new("RED-002", 4),
                DeckEntry::new("RED-003", 2),
            ],
            &["RED-002"],
        );
        let summary = result.summary();
        assert_eq!(summary.full, 3);
        assert_eq!(summary.unsupported, 4);
        assert_eq!(summary.total(), 7);
        assert!(!summary.is_complete());
    }

    /// A pack that has not been fetched is unknown, not unsupported: fetching
    /// it may well make the deck playable, and the two need different advice.
    #[test]
    fn an_unresolved_card_is_unknown_rather_than_unsupported() {
        let result = compat(
            &[DeckEntry::new("RED-LDR", 1), DeckEntry::new("OP99-999", 3)],
            &[],
        );
        let summary = result.summary();
        assert_eq!(summary.unknown, 3);
        assert_eq!(summary.unsupported, 0);
    }

    #[test]
    fn problems_lists_only_what_does_not_play_as_printed() {
        let result = compat(
            &[
                DeckEntry::new("RED-LDR", 1),
                DeckEntry::new("RED-002", 4),
                DeckEntry::new("RED-003", 4),
            ],
            &["RED-003"],
        );
        let problems: Vec<_> = result.problems().map(|c| c.number.as_str()).collect();
        assert_eq!(problems, ["RED-003"]);
        assert_eq!(
            result.cards[2].support.reason(),
            Some("card script unavailable")
        );
    }

    #[test]
    fn a_fully_supported_deck_reports_complete() {
        let result = compat(
            &[DeckEntry::new("RED-LDR", 1), DeckEntry::new("RED-002", 4)],
            &[],
        );
        assert!(result.summary().is_complete());
        assert_eq!(result.problems().count(), 0);
    }
}
