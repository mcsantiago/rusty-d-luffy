//! Printed card data and the card database.
//!
//! Card data is loaded at runtime from `data/` (populated by
//! `tools/ingest/fetch_cards.py`) rather than compiled in: the text and images
//! are Bandai's copyright, so nothing is vendored into the repo or the binary.
//! Card *scripts* — our own encoding of what each card does — are compiled in
//! and keyed by card number (see the `op-cards` crate).

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::ids::CardDefId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Category {
    Leader,
    Character,
    Event,
    Stage,
    /// DON!! cards. Not part of the 50-card deck; they live in the DON!! deck.
    Don,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Color {
    Red,
    Green,
    Blue,
    Purple,
    Black,
    Yellow,
}

impl Color {
    pub fn parse(s: &str) -> Option<Color> {
        Some(match s {
            "Red" => Color::Red,
            "Green" => Color::Green,
            "Blue" => Color::Blue,
            "Purple" => Color::Purple,
            "Black" => Color::Black,
            "Yellow" => Color::Yellow,
            _ => return None,
        })
    }
}

/// Keyword effects (comprehensive rules 10-1). Distinct from keywords that
/// merely mark timing (`[On Play]`, `[When Attacking]`, …), which are part of a
/// card's script rather than a standing property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Keyword {
    /// 10-1-1. May attack the turn it is played.
    Rush,
    /// 10-1-2. Deals 2 damage to a Leader instead of 1.
    DoubleAttack,
    /// 10-1-3. Damaged life card is trashed instead of going to hand; no Trigger.
    Banish,
    /// 10-1-4. May rest to become the new target of an attack.
    Blocker,
    /// 10-1-7. Cannot be blocked.
    Unblockable,
}

impl Keyword {
    /// Parses the keyword list punk-records extracts from the card's text box.
    /// Timing keywords are intentionally not represented here.
    pub fn parse(s: &str) -> Option<Keyword> {
        Some(match s {
            "Rush" => Keyword::Rush,
            "Double Attack" => Keyword::DoubleAttack,
            "Banish" => Keyword::Banish,
            "Blocker" => Keyword::Blocker,
            "Unblockable" => Keyword::Unblockable,
            _ => return None,
        })
    }
}

/// The printed face of a card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardDef {
    /// Card number, e.g. `"ST01-001"`. This is the key card text references
    /// (2-14-3) and the deck-building 4-copy limit key (2-14-2).
    pub number: String,
    pub name: String,
    pub category: Category,
    pub colors: Vec<Color>,
    /// Play cost. Meaningless for Leaders — see [`CardDef::life`].
    pub cost: u8,
    /// Life value, Leaders only (2-9-3).
    pub life: Option<u8>,
    pub power: Option<i32>,
    /// Counter value (2-10). Characters only.
    pub counter: Option<i32>,
    /// Trait list, e.g. `{Straw Hat Crew}`. Card text filters on these.
    pub types: Vec<String>,
    /// Attribute, e.g. `Strike`, `Slash`.
    pub attributes: Vec<String>,
    /// Standing keyword effects printed on the card.
    pub keywords: Vec<Keyword>,
    /// Raw rules text, for display and for authoring scripts against.
    pub effect: Option<String>,
    /// Raw `[Trigger]` text, if the card has one (2-11).
    pub trigger: Option<String>,
}

impl CardDef {
    pub fn has_type(&self, ty: &str) -> bool {
        self.types.iter().any(|t| t == ty)
    }

    pub fn has_keyword(&self, kw: Keyword) -> bool {
        self.keywords.contains(&kw)
    }

    /// The synthetic DON!! card. Every DON!! is identical and has no printed
    /// characteristics beyond being a DON!! card.
    pub fn don() -> CardDef {
        CardDef {
            number: "DON".to_string(),
            name: "DON!!".to_string(),
            category: Category::Don,
            colors: Vec::new(),
            cost: 0,
            life: None,
            power: None,
            counter: None,
            types: Vec::new(),
            attributes: Vec::new(),
            keywords: Vec::new(),
            effect: None,
            trigger: None,
        }
    }
}

/// The shape punk-records writes per card. Deliberately mirrors the upstream
/// JSON so ingestion stays a dumb mapping.
#[derive(Debug, Deserialize)]
struct RawCard {
    id: String,
    name: String,
    category: String,
    colors: Vec<String>,
    cost: Option<i64>,
    power: Option<i64>,
    counter: Option<i64>,
    #[serde(default)]
    types: Vec<String>,
    #[serde(default)]
    attributes: Vec<String>,
    effect: Option<String>,
    trigger: Option<String>,
}

/// Whether a card id from upstream names an alternate printing rather than a
/// distinct card.
///
/// Upstream gives every alternate art its own id by suffixing the card number:
/// `OP01-016_p1` (parallel art) and `EB01-006_r1`. These are the *same card* —
/// they share printed characteristics and, critically, the card number that the
/// four-copy deck-construction limit counts against (5-1-2-3). Registering them
/// as separate defs would let a deck run four of each and field eight, so they
/// are dropped at load; every variant has a base card, making this lossless.
pub fn is_art_variant(number: &str) -> bool {
    number.contains('_')
}

#[derive(Debug, thiserror::Error)]
pub enum CardDbError {
    #[error("card data directory not found at {0}\n\nRun: python3 tools/ingest/fetch_cards.py")]
    DataMissing(String),
    #[error("io error reading {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("malformed card json at {path}: {source}")]
    Json {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("unknown card category {category:?} on card {number}")]
    UnknownCategory { number: String, category: String },
}

/// All printed cards known to this process, plus the synthetic DON!! card.
#[derive(Debug, Clone, Default)]
pub struct CardDb {
    defs: Vec<CardDef>,
    by_number: BTreeMap<String, CardDefId>,
}

impl CardDb {
    /// A database containing only the DON!! card. Useful for kernel tests that
    /// build their own synthetic cards.
    pub fn empty() -> CardDb {
        let mut db = CardDb::default();
        db.insert(CardDef::don());
        db
    }

    /// Loads every `*.json` under `dir` recursively (i.e. `data/cards`).
    pub fn load_dir(dir: impl AsRef<Path>) -> Result<CardDb, CardDbError> {
        let dir = dir.as_ref();
        if !dir.exists() {
            return Err(CardDbError::DataMissing(dir.display().to_string()));
        }

        let mut db = CardDb::empty();
        let mut files = Vec::new();
        collect_json(dir, &mut files)?;
        // Sort so load order — and therefore CardDefId assignment — is stable.
        files.sort();

        for path in files {
            let text = std::fs::read_to_string(&path).map_err(|source| CardDbError::Io {
                path: path.display().to_string(),
                source,
            })?;
            let raw: RawCard = serde_json::from_str(&text).map_err(|source| CardDbError::Json {
                path: path.display().to_string(),
                source,
            })?;
            // Checked on the parsed id rather than the filename, so a variant is
            // dropped however the file happens to be named.
            if is_art_variant(&raw.id) {
                continue;
            }
            db.insert(convert(raw)?);
        }
        Ok(db)
    }

    pub fn insert(&mut self, def: CardDef) -> CardDefId {
        if let Some(&existing) = self.by_number.get(&def.number) {
            self.defs[existing.index()] = def;
            return existing;
        }
        let id = CardDefId(self.defs.len() as u32);
        self.by_number.insert(def.number.clone(), id);
        self.defs.push(def);
        id
    }

    pub fn get(&self, id: CardDefId) -> &CardDef {
        &self.defs[id.index()]
    }

    pub fn by_number(&self, number: &str) -> Option<CardDefId> {
        self.by_number.get(number).copied()
    }

    /// The synthetic DON!! def, always present.
    pub fn don(&self) -> CardDefId {
        self.by_number("DON").expect("DON!! def is always inserted")
    }

    pub fn len(&self) -> usize {
        self.defs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (CardDefId, &CardDef)> {
        self.defs
            .iter()
            .enumerate()
            .map(|(i, d)| (CardDefId(i as u32), d))
    }
}

fn collect_json(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> Result<(), CardDbError> {
    let entries = std::fs::read_dir(dir).map_err(|source| CardDbError::Io {
        path: dir.display().to_string(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| CardDbError::Io {
            path: dir.display().to_string(),
            source,
        })?;
        let path = entry.path();
        if path.is_dir() {
            collect_json(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "json") {
            out.push(path);
        }
    }
    Ok(())
}

fn convert(raw: RawCard) -> Result<CardDef, CardDbError> {
    let category = match raw.category.as_str() {
        "Leader" => Category::Leader,
        "Character" => Category::Character,
        "Event" => Category::Event,
        "Stage" => Category::Stage,
        "Don" | "DON" | "DON!!" => Category::Don,
        other => {
            return Err(CardDbError::UnknownCategory {
                number: raw.id,
                category: other.to_string(),
            })
        }
    };

    // Upstream stores a Leader's Life value in the `cost` field; Leaders have no
    // play cost (2-9-3). Everything else uses `cost` as printed.
    let (cost, life) = match category {
        Category::Leader => (0, raw.cost.map(|c| c as u8)),
        _ => (raw.cost.unwrap_or(0) as u8, None),
    };

    let effect = raw.effect.filter(|s| !s.is_empty());
    let keywords = effect
        .as_deref()
        .map(scan_keywords)
        .unwrap_or_default();

    Ok(CardDef {
        number: raw.id,
        name: raw.name,
        category,
        colors: raw.colors.iter().filter_map(|c| Color::parse(c)).collect(),
        cost,
        life,
        power: raw.power.map(|p| p as i32),
        counter: raw.counter.map(|c| c as i32),
        types: raw.types,
        attributes: raw.attributes,
        keywords,
        effect,
        trigger: raw.trigger.filter(|s| !s.is_empty()),
    })
}

/// Pulls standing keyword effects out of rules text.
///
/// Only unconditional keywords count as printed properties. A keyword gated on
/// a condition — `[DON!! x2] This Character gains [Rush].` — is a permanent
/// effect the card's script grants, not something the printed card has, so it is
/// skipped here and handled by the script.
fn scan_keywords(effect: &str) -> Vec<Keyword> {
    let mut out = Vec::new();
    for line in effect.split("<br>") {
        let line = line.trim();
        // A leading condition or timing marker means the keyword is granted
        // conditionally rather than printed on the card.
        let conditional = line.starts_with("[DON!!")
            || line.starts_with("[Your Turn]")
            || line.starts_with("[Opponent's Turn]")
            || line.contains("gains [");
        if conditional {
            continue;
        }
        for kw in [
            ("[Rush]", Keyword::Rush),
            ("[Double Attack]", Keyword::DoubleAttack),
            ("[Banish]", Keyword::Banish),
            ("[Blocker]", Keyword::Blocker),
            ("[Unblockable]", Keyword::Unblockable),
        ] {
            if line.starts_with(kw.0) && !out.contains(&kw.1) {
                out.push(kw.1);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leader_cost_field_is_life() {
        let raw = RawCard {
            id: "ST01-001".into(),
            name: "Monkey.D.Luffy".into(),
            category: "Leader".into(),
            colors: vec!["Red".into()],
            cost: Some(5),
            power: Some(5000),
            counter: None,
            types: vec!["Straw Hat Crew".into()],
            attributes: vec!["Strike".into()],
            effect: None,
            trigger: None,
        };
        let def = convert(raw).unwrap();
        assert_eq!(def.life, Some(5));
        assert_eq!(def.cost, 0);
    }

    #[test]
    fn alternate_printings_are_recognised() {
        // Upstream's two variant families.
        assert!(is_art_variant("OP01-016_p1"));
        assert!(is_art_variant("OP01-016_p12"));
        assert!(is_art_variant("EB01-006_r1"));
        // Real card numbers, including the synthetic DON!! def.
        assert!(!is_art_variant("OP01-016"));
        assert!(!is_art_variant("ST01-001"));
        assert!(!is_art_variant("P-001"));
        assert!(!is_art_variant("DON"));
    }

    #[test]
    fn printed_keywords_exclude_conditional_grants() {
        // ST01-012 prints [Rush]; ST01-004 only gains it with [DON!! x2].
        assert_eq!(
            scan_keywords("[Rush] (This card can attack on the turn in which it is played.)<br>[DON!! x2] [When Attacking] Your opponent cannot activate [Blocker] during this battle."),
            vec![Keyword::Rush]
        );
        assert_eq!(
            scan_keywords("[DON!! x2] This Character gains [Rush].<br>(This card can attack on the turn in which it is played.)"),
            vec![]
        );
        assert_eq!(
            scan_keywords("[Blocker] (After your opponent declares an attack, you may rest this card to make it the new target of the attack.)"),
            vec![Keyword::Blocker]
        );
    }
}
