//! Card scripts: what each printed card actually does.
//!
//! `op-core` knows the rules but nothing about individual cards; this crate
//! supplies the [`op_core::script::ScriptSource`] that closes the gap. Scripts
//! are keyed by card number and resolved against a loaded
//! [`op_core::card::CardDb`], so a card with no script simply behaves as a
//! vanilla body — which is correct for the many cards that have no text.

pub mod dsl;
pub mod sets;

use op_core::card::CardDb;
use op_core::ids::CardDefId;
use op_core::script::{CardScript, ScriptSource};

/// Scripts resolved against a particular card database.
pub struct Cards {
    /// Indexed by `CardDefId`, so lookup during resolution is an array access.
    scripts: Vec<CardScript>,
    /// Card numbers that have a script but are absent from the database.
    missing: Vec<String>,
    empty: CardScript,
}

impl Cards {
    /// Binds every known script to `db`.
    pub fn new(db: &CardDb) -> Cards {
        let mut scripts = vec![CardScript::default(); db.len()];
        let mut missing = Vec::new();

        for (number, script) in all_scripts() {
            match db.by_number(number) {
                Some(def) => scripts[def.index()] = script,
                None => missing.push(number.to_string()),
            }
        }

        Cards {
            scripts,
            missing,
            empty: CardScript::default(),
        }
    }

    /// Card numbers this crate scripts that the loaded database does not
    /// contain — usually a sign `data/` was fetched for fewer packs.
    pub fn missing(&self) -> &[String] {
        &self.missing
    }

    /// How many cards in `db` have a script.
    pub fn scripted_count(&self) -> usize {
        self.scripts.iter().filter(|s| !s.is_vanilla()).count()
    }
}

impl ScriptSource for Cards {
    fn script(&self, def: CardDefId) -> &CardScript {
        self.scripts.get(def.index()).unwrap_or(&self.empty)
    }
}

/// Every script this crate provides.
pub fn all_scripts() -> Vec<(&'static str, CardScript)> {
    let mut out = Vec::new();
    out.extend(sets::st01::scripts());
    out.extend(sets::st02::scripts());
    out.extend(sets::st06::scripts());
    out
}

/// Cards whose entire text is a printed keyword the database already carries,
/// so a script would add nothing.
///
/// Cards with *no* text at all are not listed here — the coverage report
/// detects those from the card data, which is the only approach that scales to
/// the full pool. This list exists for the cases that can't be detected: text
/// is present, but it is purely `[Blocker]` and its reminder sentence.
pub const KEYWORD_ONLY: &[&str] = &[
    "ST01-006", // [Blocker]
    "ST02-004", // [Blocker]
    "ST06-007", // [Blocker]
];
