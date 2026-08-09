//! The built-in decklists, in one place.
//!
//! Every client and test suite reads these, so a set is added once. This crate
//! is the home because a deck is only playable if its cards are scripted.

use op_core::DeckList;

/// A built-in deck: how it is named on the command line, how it reads in a
/// menu, and the list itself.
pub struct Deck {
    /// Canonical id, e.g. `"ST01"`. What a UI should send back.
    pub id: &'static str,
    /// Display name, e.g. `"ST-01 Straw Hat Crew"`.
    pub name: &'static str,
    /// Extra names accepted on the command line.
    pub aliases: &'static [&'static str],
    build: fn() -> DeckList,
}

impl Deck {
    /// The decklist itself.
    pub fn list(&self) -> DeckList {
        (self.build)()
    }
}

/// Every built-in deck, in menu order. Adding a set means adding one row.
pub const ALL: &[Deck] = &[
    Deck {
        id: "ST01",
        name: "ST-01 Straw Hat Crew",
        aliases: &["ST-01", "STRAWHAT"],
        build: st01,
    },
    Deck {
        id: "ST02",
        name: "ST-02 Worst Generation",
        aliases: &["ST-02", "WORSTGEN"],
        build: st02,
    },
    Deck {
        id: "ST04",
        name: "ST-04 Animal Kingdom Pirates",
        aliases: &["ST-04", "ANIMALKINGDOM"],
        build: st04,
    },
    Deck {
        id: "ST06",
        name: "ST-06 Absolute Justice",
        aliases: &["ST-06", "NAVY"],
        build: st06,
    },
    Deck {
        id: "ST08",
        name: "ST-08 Monkey D. Luffy",
        aliases: &["ST-08", "LUFFY"],
        build: st08,
    },
];

/// Looks a deck up by id or alias, case-insensitively.
///
/// Returns `None` rather than falling back to a default: a caller that asks for
/// a deck which does not exist has a bug or a typo, and silently handing back
/// ST-01 hides both.
pub fn find(name: &str) -> Option<&'static Deck> {
    let want = name.trim().to_ascii_uppercase();
    ALL.iter()
        .find(|d| d.id == want || d.aliases.iter().any(|a| *a == want))
}

/// The decklist for `name`, by id or alias.
pub fn by_name(name: &str) -> Option<DeckList> {
    find(name).map(|d| d.list())
}

/// The official ST-01 Straw Hat Crew decklist.
pub fn st01() -> DeckList {
    build(
        "ST01-001",
        &[
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
        ],
    )
}

/// The official ST-02 Worst Generation decklist.
pub fn st02() -> DeckList {
    build(
        "ST02-001",
        &[
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
        ],
    )
}

/// ST-04 Animal Kingdom Pirates. A legal 50-card build, not the printed list.
pub fn st04() -> DeckList {
    build(
        "ST04-001",
        &[
            ("ST04-002", 4),
            ("ST04-003", 2),
            ("ST04-004", 2),
            ("ST04-005", 4),
            ("ST04-006", 4),
            ("ST04-007", 4),
            ("ST04-008", 4),
            ("ST04-009", 4),
            ("ST04-010", 4),
            ("ST04-011", 4),
            ("ST04-012", 4),
            ("ST04-013", 4),
            ("ST04-014", 2),
            ("ST04-015", 2),
            ("ST04-016", 2),
        ],
    )
}

/// ST-06 Absolute Justice. A legal 50-card build, not the printed list.
pub fn st06() -> DeckList {
    build(
        "ST06-001",
        &[
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
        ],
    )
}

/// ST-08 Monkey D. Luffy. A legal 50-card build, not the printed list.
pub fn st08() -> DeckList {
    build(
        "ST08-001",
        &[
            ("ST08-002", 4),
            ("ST08-003", 4),
            ("ST08-004", 4),
            ("ST08-005", 2),
            ("ST08-006", 4),
            ("ST08-007", 4),
            ("ST08-008", 4),
            ("ST08-009", 4),
            ("ST08-010", 2),
            ("ST08-011", 4),
            ("ST08-012", 4),
            ("ST08-013", 2),
            ("ST08-014", 4),
            ("ST08-015", 4),
        ],
    )
}

/// Expands `(number, count)` pairs into the flat list the engine wants.
///
/// The order is load-bearing: setup assigns instance ids by walking the list,
/// so a regrouped list produces different ids and a different game from the
/// same seed.
fn build(leader: &str, spec: &[(&str, usize)]) -> DeckList {
    let mut cards = Vec::new();
    for (number, n) in spec {
        for _ in 0..*n {
            cards.push(number.to_string());
        }
    }
    DeckList {
        leader: leader.into(),
        cards,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 5-1-2 and 5-1-2-3: 50 cards, at most 4 of any number. `Game::new`
    /// enforces both, but a deck that fails here fails at the menu rather than
    /// in a test that happens to pick it.
    #[test]
    fn every_builtin_deck_is_legal() {
        for deck in ALL {
            let list = deck.list();
            assert_eq!(list.cards.len(), 50, "{} is not 50 cards", deck.id);
            for number in &list.cards {
                let n = list.cards.iter().filter(|c| *c == number).count();
                assert!(n <= 4, "{} has {n} copies of {number}", deck.id);
            }
        }
    }

    /// Ids and aliases must not collide, or `find` would silently prefer
    /// whichever row came first.
    #[test]
    fn no_deck_name_is_ambiguous() {
        let mut seen: Vec<&str> = Vec::new();
        for deck in ALL {
            seen.push(deck.id);
            seen.extend(deck.aliases);
        }
        let mut sorted = seen.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), seen.len(), "a deck id or alias is repeated");
    }

    #[test]
    fn lookup_accepts_ids_aliases_and_any_case() {
        assert_eq!(find("ST01").unwrap().id, "ST01");
        assert_eq!(find("st-01").unwrap().id, "ST01");
        assert_eq!(find("StrawHat").unwrap().id, "ST01");
        assert_eq!(find("  ST08  ").unwrap().id, "ST08");
        assert!(find("ST99").is_none());
    }

    /// The bug this module exists to prevent: a scripted set that never
    /// reaches a menu. If a `sets/` module ships, it belongs here too.
    #[test]
    fn every_scripted_set_is_offered_as_a_deck() {
        let mut scripted: Vec<String> = crate::all_scripts()
            .iter()
            .map(|(number, _)| number[..4].to_string())
            .collect();
        scripted.sort_unstable();
        scripted.dedup();
        for set in &scripted {
            assert!(
                ALL.iter().any(|d| d.id == set.as_str()),
                "{set} has card scripts but no built-in deck; add it to decks::ALL"
            );
        }
    }
}
