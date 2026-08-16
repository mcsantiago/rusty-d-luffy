//! Saved decks on disk.
//!
//! One JSON file per deck, named for its id. One file rather than a single
//! registry because the failure modes are better: a deck that fails to parse
//! costs its owner that deck, not the collection, and two processes saving
//! different decks cannot lose each other's work.
//!
//! A saved deck holds card numbers and counts, never a copy of the card's
//! printed data. The [`op_core::card::CardDb`] is the only source of that, and
//! a deck carrying its own would go stale the first time a card's data was
//! re-fetched.
//!
//! **Entry order is preserved end to end.** It survives the file, because JSON
//! arrays are ordered, and it survives [`SavedDeck::to_decklist`]. Setup
//! assigns instance ids by walking the decklist, so regrouping a deck between
//! sessions would change what the same seed plays — and with it every session
//! log recorded against that deck.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::DeckEntry;

/// A deck's stable identity, and its filename.
///
/// Constrained to characters that are safe in a path on every platform, which
/// is not decoration: the id is derived from a user-supplied name, and an id
/// permitting `/` or `..` would let a deck called `../../etc/passwd` decide
/// where the file lands.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DeckId(String);

impl DeckId {
    /// Accepts an already-valid id.
    pub fn new(id: impl Into<String>) -> Result<DeckId, StoreError> {
        let id = id.into();
        let valid = !id.is_empty()
            && id.len() <= 64
            && id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !valid {
            return Err(StoreError::InvalidId(id));
        }
        Ok(DeckId(id))
    }

    /// Derives an id from a deck name: lowercase, runs of anything else
    /// collapsed to a single dash.
    ///
    /// Never fails — a name of pure punctuation, or of a script with no ASCII
    /// at all, falls back to `deck` and is uniquified by the store.
    pub fn from_name(name: &str) -> DeckId {
        let mut out = String::new();
        for ch in name.chars() {
            if ch.is_ascii_alphanumeric() {
                out.push(ch.to_ascii_lowercase());
            } else if !out.ends_with('-') {
                out.push('-');
            }
        }
        let slug = out.trim_matches('-');
        if slug.is_empty() {
            return DeckId("deck".to_string());
        }
        DeckId(
            slug.chars()
                .take(56)
                .collect::<String>()
                .trim_end_matches('-')
                .to_string(),
        )
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DeckId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for DeckId {
    type Error = StoreError;
    fn try_from(value: String) -> Result<DeckId, StoreError> {
        DeckId::new(value)
    }
}

impl From<DeckId> for String {
    fn from(id: DeckId) -> String {
        id.0
    }
}

/// A deck as it is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedDeck {
    pub id: DeckId,
    /// Free text, and not unique. The id is what identifies a deck.
    pub name: String,
    /// The Leader's card number.
    pub leader: String,
    /// The 50, as counts. Order is meaningful; see the module docs.
    pub cards: Vec<DeckEntry>,
}

impl SavedDeck {
    /// The deck as the engine wants it.
    pub fn to_decklist(&self) -> op_core::DeckList {
        crate::expand(&self.leader, &self.cards)
    }

    /// The deck in the interoperable text format.
    pub fn to_text(&self) -> String {
        crate::text::write(&self.leader, &self.cards)
    }

    /// Copies in the deck, the Leader excluded — what 5-1-2 counts.
    pub fn size(&self) -> u32 {
        self.cards.iter().map(|e| e.quantity).sum()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{0:?} is not a valid deck id")]
    InvalidId(String),
    #[error("no saved deck {0}")]
    NotFound(DeckId),
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("malformed deck file {path}: {source}")]
    Json {
        path: String,
        #[source]
        source: serde_json::Error,
    },
}

/// A directory of saved decks.
///
/// The directory is the caller's choice, so this crate holds no opinion about
/// where an application keeps its data — the desktop client has a Tauri app
/// data directory and the terminal client does not.
#[derive(Debug, Clone)]
pub struct DeckStore {
    dir: PathBuf,
}

impl DeckStore {
    /// Opens `dir`, creating it if it does not exist.
    pub fn open(dir: impl Into<PathBuf>) -> Result<DeckStore, StoreError> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir).map_err(|source| StoreError::Io {
            path: dir.display().to_string(),
            source,
        })?;
        Ok(DeckStore { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, id: &DeckId) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    /// Every saved deck, by name then id so the order a menu shows is stable.
    ///
    /// A file that does not parse is skipped rather than failing the listing:
    /// one corrupt deck should not make the other twenty unreachable.
    pub fn list(&self) -> Result<Vec<SavedDeck>, StoreError> {
        let entries = std::fs::read_dir(&self.dir).map_err(|source| StoreError::Io {
            path: self.dir.display().to_string(),
            source,
        })?;

        let mut decks = Vec::new();
        for entry in entries {
            let path = entry
                .map_err(|source| StoreError::Io {
                    path: self.dir.display().to_string(),
                    source,
                })?
                .path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            if let Ok(deck) = read(&path) {
                decks.push(deck);
            }
        }

        decks.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        Ok(decks)
    }

    pub fn load(&self, id: &DeckId) -> Result<SavedDeck, StoreError> {
        let path = self.path(id);
        if !path.exists() {
            return Err(StoreError::NotFound(id.clone()));
        }
        read(&path)
    }

    pub fn exists(&self, id: &DeckId) -> bool {
        self.path(id).exists()
    }

    /// Writes `deck`, replacing any deck with the same id.
    ///
    /// Written to a temporary file and renamed, so an interrupted save leaves
    /// the previous deck intact rather than a half-written file that no longer
    /// parses.
    pub fn save(&self, deck: &SavedDeck) -> Result<(), StoreError> {
        let path = self.path(&deck.id);
        let temp = path.with_extension("json.tmp");

        let json = serde_json::to_string_pretty(deck).map_err(|source| StoreError::Json {
            path: path.display().to_string(),
            source,
        })?;
        std::fs::write(&temp, json).map_err(|source| StoreError::Io {
            path: temp.display().to_string(),
            source,
        })?;
        std::fs::rename(&temp, &path).map_err(|source| StoreError::Io {
            path: path.display().to_string(),
            source,
        })
    }

    /// Saves a new deck under an id derived from its name.
    ///
    /// The id is uniquified against what is already stored, so two decks called
    /// "Red Zoro" both save rather than the second overwriting the first.
    pub fn create(
        &self,
        name: &str,
        leader: &str,
        cards: Vec<DeckEntry>,
    ) -> Result<SavedDeck, StoreError> {
        let deck = SavedDeck {
            id: self.unique_id(&DeckId::from_name(name)),
            name: name.to_string(),
            leader: leader.to_string(),
            cards,
        };
        self.save(&deck)?;
        Ok(deck)
    }

    pub fn delete(&self, id: &DeckId) -> Result<(), StoreError> {
        let path = self.path(id);
        if !path.exists() {
            return Err(StoreError::NotFound(id.clone()));
        }
        std::fs::remove_file(&path).map_err(|source| StoreError::Io {
            path: path.display().to_string(),
            source,
        })
    }

    /// Renames a deck. The id does not move with the name: it is the deck's
    /// identity, and changing it would break anything already pointing at it.
    pub fn rename(&self, id: &DeckId, name: &str) -> Result<SavedDeck, StoreError> {
        let mut deck = self.load(id)?;
        deck.name = name.to_string();
        self.save(&deck)?;
        Ok(deck)
    }

    /// Copies a deck under a new id.
    pub fn duplicate(&self, id: &DeckId) -> Result<SavedDeck, StoreError> {
        let source = self.load(id)?;
        self.create(
            &format!("{} (copy)", source.name),
            &source.leader,
            source.cards.clone(),
        )
    }

    /// `base`, or the first `base-2`, `base-3`, … not already taken.
    fn unique_id(&self, base: &DeckId) -> DeckId {
        if !self.exists(base) {
            return base.clone();
        }
        for n in 2..1000 {
            if let Ok(candidate) = DeckId::new(format!("{base}-{n}")) {
                if !self.exists(&candidate) {
                    return candidate;
                }
            }
        }
        // A thousand decks of one name is not a case worth a fallible signature;
        // overwriting the last is the least surprising outcome left.
        base.clone()
    }
}

fn read(path: &Path) -> Result<SavedDeck, StoreError> {
    let text = std::fs::read_to_string(path).map_err(|source| StoreError::Io {
        path: path.display().to_string(),
        source,
    })?;
    serde_json::from_str(&text).map_err(|source| StoreError::Json {
        path: path.display().to_string(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("op-deck-test-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn cards() -> Vec<DeckEntry> {
        vec![DeckEntry::new("RED-002", 4), DeckEntry::new("RED-003", 2)]
    }

    #[test]
    fn a_saved_deck_survives_a_reopen() {
        let store = DeckStore::open(temp_dir("reopen")).unwrap();
        let saved = store.create("Red Zoro", "RED-LDR", cards()).unwrap();

        let reopened = DeckStore::open(store.dir()).unwrap();
        assert_eq!(reopened.load(&saved.id).unwrap(), saved);
        assert_eq!(reopened.list().unwrap(), [saved]);
    }

    /// Entry order decides instance ids at setup, so a deck that came back
    /// regrouped would play a different game from the same seed.
    #[test]
    fn a_reloaded_deck_keeps_its_entry_order() {
        let store = DeckStore::open(temp_dir("order")).unwrap();
        let entries = vec![
            DeckEntry::new("RED-009", 1),
            DeckEntry::new("RED-002", 4),
            DeckEntry::new("RED-003", 2),
        ];
        let saved = store.create("Order", "RED-LDR", entries.clone()).unwrap();
        assert_eq!(store.load(&saved.id).unwrap().cards, entries);
        assert_eq!(
            saved.to_decklist().cards,
            ["RED-009", "RED-002", "RED-002", "RED-002", "RED-002", "RED-003", "RED-003"]
        );
    }

    #[test]
    fn two_decks_of_one_name_both_save() {
        let store = DeckStore::open(temp_dir("collide")).unwrap();
        let first = store.create("Red Zoro", "RED-LDR", cards()).unwrap();
        let second = store.create("Red Zoro", "RED-LDR", cards()).unwrap();
        assert_eq!(first.id.as_str(), "red-zoro");
        assert_eq!(second.id.as_str(), "red-zoro-2");
        assert_eq!(store.list().unwrap().len(), 2);
    }

    #[test]
    fn rename_keeps_the_id_so_references_survive() {
        let store = DeckStore::open(temp_dir("rename")).unwrap();
        let deck = store.create("Red Zoro", "RED-LDR", cards()).unwrap();
        let renamed = store.rename(&deck.id, "Green Bonney").unwrap();
        assert_eq!(renamed.id, deck.id);
        assert_eq!(store.load(&deck.id).unwrap().name, "Green Bonney");
    }

    #[test]
    fn duplicate_and_delete() {
        let store = DeckStore::open(temp_dir("dup")).unwrap();
        let deck = store.create("Red Zoro", "RED-LDR", cards()).unwrap();
        let copy = store.duplicate(&deck.id).unwrap();
        assert_eq!(copy.name, "Red Zoro (copy)");
        assert_eq!(copy.cards, deck.cards);
        assert_ne!(copy.id, deck.id);

        store.delete(&deck.id).unwrap();
        assert!(store.load(&deck.id).is_err());
        assert_eq!(store.list().unwrap(), [copy]);
    }

    /// The id becomes a filename, so a name that walks out of the directory
    /// must not produce one.
    #[test]
    fn a_hostile_deck_name_cannot_escape_the_directory() {
        let id = DeckId::from_name("../../etc/passwd");
        assert_eq!(id.as_str(), "etc-passwd");
        assert!(DeckId::new("../../etc/passwd").is_err());
        assert!(DeckId::new("..").is_err());
        assert!(DeckId::new("").is_err());
        assert!(DeckId::new("Red/Zoro").is_err());
    }

    #[test]
    fn a_name_with_no_ascii_still_yields_a_usable_id() {
        assert_eq!(DeckId::from_name("!!!").as_str(), "deck");
        assert_eq!(DeckId::from_name("").as_str(), "deck");
        assert_eq!(DeckId::from_name("  Red   Zoro  ").as_str(), "red-zoro");
    }

    /// One unreadable file should not take the rest of the collection with it.
    #[test]
    fn a_corrupt_deck_file_is_skipped_rather_than_failing_the_listing() {
        let store = DeckStore::open(temp_dir("corrupt")).unwrap();
        let good = store.create("Good", "RED-LDR", cards()).unwrap();
        std::fs::write(store.dir().join("broken.json"), "{ not json").unwrap();
        assert_eq!(store.list().unwrap(), [good]);
    }

    #[test]
    fn a_saved_deck_exports_as_the_text_format() {
        let store = DeckStore::open(temp_dir("export")).unwrap();
        let deck = store.create("Red Zoro", "RED-LDR", cards()).unwrap();
        assert_eq!(deck.to_text(), "1 RED-LDR\n4 RED-002\n2 RED-003\n");
        assert_eq!(deck.size(), 6);
    }
}
