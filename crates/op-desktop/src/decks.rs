//! Deck selection, import and storage for the desktop client.
//!
//! The policy layer over `op-deck`: which decks the menu offers, what an
//! imported list is allowed to become, and when a deck may be played. The Tauri
//! commands in `main.rs` are thin wrappers over this so the decisions are
//! testable without a window.
//!
//! Two of those decisions are worth stating plainly, because they are the ones
//! the issue asks for and they cut in opposite directions:
//!
//! * **Legality gates saving.** An illegal deck cannot be played anywhere, and
//!   storing one only defers the error to the moment a game is being started.
//! * **Engine support does not.** A legal deck full of unscripted cards is a
//!   real deck this build has not caught up with. It saves, it warns, and it is
//!   stopped at *launch* instead — where the player can override.

use op_cards::Cards;
use op_core::card::CardDb;
use op_core::DeckList;
use op_deck::store::{DeckId, DeckStore};
use op_deck::{collapse, compat, legality, resolve, text, DeckEntry};
use serde::Serialize;

/// A deck as the setup menu needs it: what to show, and what to send back.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DeckChoice {
    pub id: String,
    pub name: String,
    /// `"builtin"` or `"saved"`, so the menu can group them and offer delete
    /// and export only on the ones that own a file.
    pub source: &'static str,
    /// Whether every card plays as printed. `None` when card data is not
    /// loaded and the question cannot be answered.
    pub supported: Option<bool>,
}

/// What an import produced, in the four layers `op-deck` keeps apart.
///
/// All four are always reported. A pasted list can be unreadable in places,
/// name cards we do not have, break a construction rule, and contain unscripted
/// cards all at once — and a player fixing it needs to see every one, not the
/// first that happened to be checked.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct ImportReport {
    /// The id it was saved under, or `None` if it was not saved.
    pub saved: Option<String>,
    pub name: String,
    /// The Leader's card number, once one is identified.
    pub leader: Option<String>,
    /// Deck cards the list asks for, the Leader excluded.
    pub size: u32,
    pub parse_problems: Vec<String>,
    pub unknown: Vec<String>,
    pub legality: Vec<String>,
    /// Cards this build cannot play as printed, with the reason.
    pub unsupported: Vec<String>,
    pub supported_copies: u32,
    pub total_copies: u32,
}

/// Card data, once loaded. Borrowed together because every question here needs
/// both the printed cards and what we have implemented for them.
#[derive(Clone, Copy)]
pub struct Loaded<'a> {
    pub db: &'a CardDb,
    pub cards: &'a Cards,
}

/// Whether every card in `list` plays as printed on this build.
pub fn is_supported(loaded: Loaded<'_>, list: &DeckList) -> bool {
    let resolved = resolve::resolve(&entries_of(list), loaded.db);
    compat::check(&resolved, loaded.db, loaded.cards)
        .summary()
        .is_complete()
}

/// A `DeckList` as counted entries, the Leader included.
///
/// The Leader is a separate field on `DeckList` but an ordinary entry
/// everywhere in `op-deck`, and the colour rule needs it present to have
/// anything to check against.
fn entries_of(list: &DeckList) -> Vec<DeckEntry> {
    let mut entries = vec![DeckEntry::new(list.leader.clone(), 1)];
    entries.extend(collapse(&list.cards));
    entries
}

/// The decks the client may offer: the built-in lists, then the player's own.
///
/// A saved deck whose id collides with a built-in one is still listed. They
/// resolve saved-first, so the player's own deck is the one they get.
pub fn choices(store: &DeckStore, loaded: Option<Loaded<'_>>) -> Vec<DeckChoice> {
    let mut out: Vec<DeckChoice> = op_cards::decks::ALL
        .iter()
        .map(|d| DeckChoice {
            id: d.id.to_string(),
            name: d.name.to_string(),
            source: "builtin",
            supported: loaded.map(|l| is_supported(l, &d.list())),
        })
        .collect();

    for deck in store.list().unwrap_or_default() {
        out.push(DeckChoice {
            id: deck.id.to_string(),
            name: deck.name.clone(),
            source: "saved",
            supported: loaded.map(|l| is_supported(l, &deck.to_decklist())),
        });
    }

    out
}

/// Resolves a deck id from the setup menu.
///
/// Saved decks win over built-ins of the same id, so importing a deck called
/// `ST01` shadows the starter rather than being unreachable. An unknown id is
/// an error and not a fallback to some default: with saved decks in play an id
/// can genuinely stop existing — deleted in another window — and silently
/// starting a different deck than the one asked for is worse than saying so.
pub fn resolve_id(store: &DeckStore, id: &str) -> Result<DeckList, String> {
    if let Ok(deck_id) = DeckId::new(id) {
        if let Ok(deck) = store.load(&deck_id) {
            return Ok(deck.to_decklist());
        }
    }
    op_cards::decks::by_name(id).ok_or_else(|| format!("no deck {id}"))
}

/// Reads a decklist, reports on it, and saves it when it is legal.
pub fn import(
    store: &DeckStore,
    loaded: Loaded<'_>,
    name: &str,
    text: &str,
) -> Result<ImportReport, String> {
    let parsed = text::parse(text);
    let resolved = resolve::resolve(&parsed.entries, loaded.db);
    let problems = legality::check(&resolved, loaded.db);
    let compat = compat::check(&resolved, loaded.db, loaded.cards);
    let summary = compat.summary();

    // An unnamed import takes the Leader's printed name, which is what a player
    // would have typed anyway.
    let name = match name.trim() {
        "" => resolved
            .leader()
            .map(|l| loaded.db.get(l.def).name.clone())
            .unwrap_or_else(|| "Imported deck".to_string()),
        given => given.to_string(),
    };

    let mut report = ImportReport {
        name: name.clone(),
        leader: resolved.leader().map(|l| l.number.clone()),
        size: resolved.deck_size(),
        parse_problems: parsed.problems.iter().map(|p| p.to_string()).collect(),
        unknown: resolved.unknown.iter().map(|e| e.number.clone()).collect(),
        legality: problems.iter().map(|e| e.to_string()).collect(),
        unsupported: compat
            .problems()
            .map(|c| format!("{} — {}", c.number, c.support.reason().unwrap_or("unknown")))
            .collect(),
        supported_copies: summary.full,
        total_copies: summary.total(),
        saved: None,
    };

    // Unknown cards do not fail legality — the deck may be legal at a table —
    // but a deck we cannot build every card of is not one that can be played.
    if problems.is_empty() && resolved.unknown.is_empty() {
        let leader = resolved
            .leader()
            .ok_or("a legal deck has exactly one Leader")?;
        let entries: Vec<DeckEntry> = resolved
            .cards
            .iter()
            .map(|c| DeckEntry::new(c.number.clone(), c.quantity))
            .collect();
        let saved = store
            .create(&name, &leader.number, entries)
            .map_err(|e| e.to_string())?;
        report.saved = Some(saved.id.to_string());
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")
    }

    /// The fetched card database, or `None` to skip: a bare clone has no
    /// `data/`, and the suite stays green on one.
    fn card_db() -> Option<CardDb> {
        CardDb::load_dir(data_dir().join("cards")).ok()
    }

    fn store(tag: &str) -> DeckStore {
        let dir = std::env::temp_dir().join(format!("op-desktop-decks-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        DeckStore::open(dir).unwrap()
    }

    /// ST-01 as a pasted decklist.
    fn st01_text() -> String {
        let list = op_cards::decks::st01();
        text::write(&list.leader, &collapse(&list.cards))
    }

    #[test]
    fn a_legal_decklist_is_saved_and_becomes_selectable() {
        let Some(db) = card_db() else { return };
        let cards = Cards::new(&db);
        let loaded = Loaded {
            db: &db,
            cards: &cards,
        };
        let store = store("legal");

        let report = import(&store, loaded, "My Straw Hats", &st01_text()).unwrap();
        assert_eq!(report.legality, Vec::<String>::new());
        assert_eq!(report.unsupported, Vec::<String>::new());
        assert_eq!(report.size, 50);
        let id = report.saved.expect("a legal deck saves");

        // It reaches the menu, and it resolves back to the deck imported.
        let listed = choices(&store, Some(loaded));
        assert!(listed.iter().any(|c| c.id == id && c.source == "saved"));
        assert_eq!(resolve_id(&store, &id).unwrap(), op_cards::decks::st01());
    }

    /// The rule the engine never checked. An off-colour deck must not become a
    /// saved deck that fails at the moment a game is started.
    #[test]
    fn an_off_colour_deck_is_reported_and_not_saved() {
        let Some(db) = card_db() else { return };
        let cards = Cards::new(&db);
        let loaded = Loaded {
            db: &db,
            cards: &cards,
        };
        let store = store("off-colour");

        // ST-01 is red; ST03-002 is not.
        let mut list = op_cards::decks::st01();
        list.cards[0] = "ST03-002".to_string();
        let pasted = text::write(&list.leader, &collapse(&list.cards));

        let report = import(&store, loaded, "Illegal", &pasted).unwrap();
        assert!(report.saved.is_none(), "an illegal deck must not save");
        assert!(
            report.legality.iter().any(|e| e.contains("ST03-002")),
            "{:?}",
            report.legality
        );
        assert_eq!(store.list().unwrap().len(), 0);
    }

    /// A deck naming cards we do not have is not illegal, but it cannot be
    /// built either — so it is reported and withheld rather than saved.
    #[test]
    fn a_deck_with_unfetched_cards_is_not_saved() {
        let Some(db) = card_db() else { return };
        let cards = Cards::new(&db);
        let loaded = Loaded {
            db: &db,
            cards: &cards,
        };
        let store = store("unknown");

        let mut list = op_cards::decks::st01();
        list.cards[0] = "OP99-999".to_string();
        let pasted = text::write(&list.leader, &collapse(&list.cards));

        let report = import(&store, loaded, "Missing pack", &pasted).unwrap();
        assert_eq!(report.unknown, ["OP99-999"]);
        assert_eq!(report.legality, Vec::<String>::new(), "not a rules problem");
        assert!(report.saved.is_none());
    }

    #[test]
    fn an_unnamed_import_takes_the_leaders_name() {
        let Some(db) = card_db() else { return };
        let cards = Cards::new(&db);
        let loaded = Loaded {
            db: &db,
            cards: &cards,
        };
        let store = store("unnamed");

        let report = import(&store, loaded, "   ", &st01_text()).unwrap();
        assert_eq!(report.name, db.get(db.by_number("ST01-001").unwrap()).name);
    }

    /// A saved deck shadows a built-in of the same id, so a player who names
    /// their deck after a starter still gets their own.
    #[test]
    fn a_saved_deck_wins_over_a_builtin_of_the_same_id() {
        let Some(db) = card_db() else { return };
        let cards = Cards::new(&db);
        let loaded = Loaded {
            db: &db,
            cards: &cards,
        };
        let store = store("shadow");

        // ST-02's list, saved under the id the ST-01 starter uses.
        let list = op_cards::decks::st02();
        let entries = collapse(&list.cards);
        let saved = store.create("ST01", &list.leader, entries).unwrap();
        assert_eq!(saved.id.as_str(), "st01");

        // The built-in is keyed "ST01"; ids are compared case-insensitively by
        // `op_cards::decks::find`, so this is the collision that matters.
        assert_eq!(resolve_id(&store, "st01").unwrap(), list);
        let _ = loaded;
    }

    #[test]
    fn an_unknown_deck_id_is_an_error_rather_than_a_default() {
        let store = store("missing");
        assert!(resolve_id(&store, "no-such-deck").is_err());
    }
}
