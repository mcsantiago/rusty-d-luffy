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
    out
}

/// Card numbers that are deliberately vanilla — no text, or text that is
/// entirely a printed keyword the database already carries. Listed explicitly
/// so the coverage report can tell "done" apart from "not started".
pub const INTENTIONALLY_VANILLA: &[&str] = &[
    // ST-01: no card text.
    "ST01-003", "ST01-008", "ST01-009", "ST01-010",
    // ST-01: [Blocker] only, which is a printed keyword.
    "ST01-006",
    // ST-02: no card text.
    "ST02-002", "ST02-006", "ST02-011", "ST02-012",
    // ST-02: [Blocker] only.
    "ST02-004",
];
