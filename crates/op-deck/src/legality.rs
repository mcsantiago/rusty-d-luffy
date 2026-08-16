//! Deck construction rules — 5-1-2.
//!
//! Every check here is a clause of the Comprehensive Rules and nothing else.
//! Whether the engine can *play* the deck is [`crate::compat`]'s question, and
//! a deck that fails this module is illegal at a real table too.
//!
//! `Game::new` validates size and the copy limit again at setup, and must: this
//! crate is not on the path when a decklist arrives from a file or a test. What
//! it does not check is colour (5-1-2-2) or card category (5-1-2-1), so an
//! import that skipped this module would build an illegal deck and the engine
//! would play it without complaint.
//!
//! Every violation is reported, not just the first. A deck three cards over
//! with two off-colour cards should say so once, rather than over five
//! successive attempts to fix it.

use op_core::card::{CardDb, Category, Color};

use crate::resolve::ResolvedDeck;

/// 5-1-2: a deck is exactly 50 cards, the Leader not among them.
pub const DECK_SIZE: u32 = 50;

/// 5-1-2-3: at most 4 cards with the same card number.
pub const MAX_COPIES: u32 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegalityError {
    /// 5-1-2: exactly 1 Leader card.
    MissingLeader,
    MultipleLeaders {
        numbers: Vec<String>,
    },
    /// 5-1-2: exactly 50 cards.
    DeckSize {
        found: u32,
    },
    /// 5-1-2-3.
    TooManyCopies {
        number: String,
        count: u32,
    },
    /// 5-1-2-2: only cards of a colour on the Leader may be in the deck.
    OffColour {
        number: String,
        card: Vec<Color>,
        leader: Vec<Color>,
    },
    /// 5-1-2-1: a deck is Character, Event and Stage cards.
    NotADeckCard {
        number: String,
        category: Category,
    },
}

impl std::fmt::Display for LegalityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LegalityError::MissingLeader => write!(f, "missing Leader"),
            LegalityError::MultipleLeaders { numbers } => {
                write!(f, "more than one Leader: {}", numbers.join(", "))
            }
            LegalityError::DeckSize { found } => {
                write!(f, "deck contains {found}/{DECK_SIZE} required cards")
            }
            LegalityError::TooManyCopies { number, count } => write!(
                f,
                "{count} copies of {number}; no more than {MAX_COPIES} of a card number"
            ),
            LegalityError::OffColour {
                number,
                card,
                leader,
            } => write!(
                f,
                "{number} is {}; the Leader is {}",
                colours(card),
                colours(leader)
            ),
            LegalityError::NotADeckCard { number, category } => {
                write!(f, "{number} is a {category:?} card and cannot be in a deck")
            }
        }
    }
}

fn colours(colors: &[Color]) -> String {
    if colors.is_empty() {
        return "colourless".to_string();
    }
    colors
        .iter()
        .map(|c| format!("{c:?}"))
        .collect::<Vec<_>>()
        .join("/")
}

/// Every way `deck` breaks 5-1-2, in the order a reader wants them: what the
/// deck is missing, then how big it is, then card by card in decklist order.
pub fn check(deck: &ResolvedDeck, db: &CardDb) -> Vec<LegalityError> {
    let mut out = Vec::new();

    match deck.leaders.as_slice() {
        [] => out.push(LegalityError::MissingLeader),
        [_] => {}
        many => out.push(LegalityError::MultipleLeaders {
            numbers: many.iter().map(|c| c.number.clone()).collect(),
        }),
    }

    let size = deck.deck_size();
    if size != DECK_SIZE {
        out.push(LegalityError::DeckSize { found: size });
    }

    // 5-1-2-2 is only answerable against a single Leader. With none or two, the
    // colour of every card is unknowable rather than wrong, and reporting 50
    // off-colour cards would bury the error that actually needs fixing.
    let leader_colors = deck
        .leader()
        .map(|l| db.get(l.def).colors.clone())
        .unwrap_or_default();

    for card in &deck.cards {
        if card.quantity > MAX_COPIES {
            out.push(LegalityError::TooManyCopies {
                number: card.number.clone(),
                count: card.quantity,
            });
        }

        let def = db.get(card.def);
        if !matches!(
            def.category,
            Category::Character | Category::Event | Category::Stage
        ) {
            out.push(LegalityError::NotADeckCard {
                number: card.number.clone(),
                category: def.category,
            });
            // Its colour is beside the point once it cannot be in a deck at all.
            continue;
        }

        if !leader_colors.is_empty() && !def.colors.iter().any(|c| leader_colors.contains(c)) {
            out.push(LegalityError::OffColour {
                number: card.number.clone(),
                card: def.colors.clone(),
                leader: leader_colors.clone(),
            });
        }
    }

    out
}

/// Whether the deck may be played at a real table.
///
/// Unknown cards do not make a deck illegal — see [`crate::resolve`] — so a
/// deck can be legal here and still fail at setup for a card the install does
/// not have. Callers wanting "can I start a game" want this *and* an empty
/// [`ResolvedDeck::unknown`].
///
/// [`ResolvedDeck::unknown`]: crate::resolve::ResolvedDeck::unknown
pub fn is_legal(deck: &ResolvedDeck, db: &CardDb) -> bool {
    check(deck, db).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::resolve;
    use crate::tests::test_db;
    use crate::DeckEntry;

    /// A legal red deck: the Leader plus 50 red cards inside the copy limit.
    fn legal_entries() -> Vec<DeckEntry> {
        vec![
            DeckEntry::new("RED-LDR", 1),
            DeckEntry::new("RED-002", 4),
            DeckEntry::new("RED-003", 4),
            DeckEntry::new("RED-EVENT", 4),
            DeckEntry::new("RED-STAGE", 4),
            DeckEntry::new("RG-005", 4),
            DeckEntry::new("RED-006", 4),
            DeckEntry::new("RED-007", 4),
            DeckEntry::new("RED-008", 4),
            DeckEntry::new("RED-009", 4),
            DeckEntry::new("RED-010", 4),
            DeckEntry::new("RED-011", 4),
            DeckEntry::new("RED-012", 4),
            DeckEntry::new("RED-013", 2),
        ]
    }

    fn check_entries(entries: &[DeckEntry]) -> Vec<LegalityError> {
        let db = test_db();
        check(&resolve(entries, &db), &db)
    }

    #[test]
    fn a_legal_deck_reports_nothing() {
        assert_eq!(check_entries(&legal_entries()), []);
    }

    #[test]
    fn a_deck_without_a_leader_says_so() {
        let entries: Vec<_> = legal_entries().into_iter().skip(1).collect();
        assert!(check_entries(&entries).contains(&LegalityError::MissingLeader));
    }

    #[test]
    fn rule_5_1_2_a_short_deck_names_both_numbers() {
        let mut entries = legal_entries();
        entries.pop();
        let errors = check_entries(&entries);
        assert!(errors.contains(&LegalityError::DeckSize { found: 48 }));
        assert_eq!(errors[0].to_string(), "deck contains 48/50 required cards");
    }

    #[test]
    fn rule_5_1_2_3_more_than_four_copies_is_reported_with_the_count() {
        let mut entries = legal_entries();
        entries[1].quantity = 5;
        entries.last_mut().unwrap().quantity = 1;
        let errors = check_entries(&entries);
        assert!(errors.contains(&LegalityError::TooManyCopies {
            number: "RED-002".into(),
            count: 5,
        }));
    }

    /// The rule the engine's own `validate_deck` does not check, which is why
    /// an importer has to: an off-colour deck would otherwise be built, saved
    /// and played without complaint.
    #[test]
    fn rule_5_1_2_2_a_card_sharing_no_colour_with_the_leader_is_illegal() {
        let mut entries = legal_entries();
        entries[1] = DeckEntry::new("BLUE-002", 4);
        let errors = check_entries(&entries);
        assert!(
            errors.iter().any(|e| matches!(
                e,
                LegalityError::OffColour { number, .. } if number == "BLUE-002"
            )),
            "{errors:?}"
        );
    }

    /// 5-1-2-2 asks for a shared colour, not an identical one: Red/Green is
    /// legal under a Red Leader, where mono-Green is not.
    #[test]
    fn rule_5_1_2_2_one_shared_colour_is_enough() {
        // Deck size is beside the point here; only the colour verdict is read.
        let off_colour = |number: &str| {
            check_entries(&[DeckEntry::new("RED-LDR", 1), DeckEntry::new(number, 4)])
                .iter()
                .any(|e| matches!(e, LegalityError::OffColour { .. }))
        };
        assert!(
            !off_colour("RG-005"),
            "Red/Green is legal under a Red Leader"
        );
        assert!(off_colour("GREEN-002"), "mono-Green is not");
    }

    #[test]
    fn rule_5_1_2_1_a_leader_in_the_fifty_is_reported_as_a_second_leader() {
        let mut entries = legal_entries();
        entries.last_mut().unwrap().quantity = 1;
        entries.push(DeckEntry::new("BLUE-LDR", 1));
        let errors = check_entries(&entries);
        assert!(errors
            .iter()
            .any(|e| matches!(e, LegalityError::MultipleLeaders { .. })));
    }

    /// With no single Leader the colour of every card is unknowable rather than
    /// wrong; 50 off-colour errors would bury the one that needs fixing.
    #[test]
    fn colour_is_not_checked_without_exactly_one_leader() {
        let entries: Vec<_> = legal_entries().into_iter().skip(1).collect();
        let errors = check_entries(&entries);
        assert!(!errors
            .iter()
            .any(|e| matches!(e, LegalityError::OffColour { .. })));
    }

    /// Unknown cards are a resolution result, not a rules violation — the deck
    /// above is legal at a table, we simply do not have the pack.
    #[test]
    fn an_unknown_card_is_not_a_legality_error() {
        let mut entries = legal_entries();
        entries[1] = DeckEntry::new("OP99-999", 4);
        let errors = check_entries(&entries);
        assert_eq!(errors, [], "unknown cards should not fail legality");
    }
}
