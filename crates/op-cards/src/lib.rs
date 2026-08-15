//! Card scripts: what each printed card actually does.
//!
//! `op-core` knows the rules but nothing about individual cards; this crate
//! supplies the [`op_core::script::ScriptSource`] that closes the gap. Scripts
//! are keyed by card number and resolved against a loaded
//! [`op_core::card::CardDb`], so a card with no script simply behaves as a
//! vanilla body — which is correct for the many cards that have no text.

pub mod decks;
pub mod dsl;
pub mod sets;

use op_core::card::{CardDb, CardDef};
use op_core::ids::CardDefId;
use op_core::script::{CardScript, ScriptSource};
use op_core::validate::{validate_script, Diagnostic};

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

/// Why a card does, or does not, play as printed.
///
/// One classification with two consumers: the `coverage` binary, which reports
/// it per set, and [`op_deck::compat`], which reports it per deck. They were
/// always going to answer the same question, and two implementations of it
/// would drift the first time a new way to be unplayable appeared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Coverage {
    /// Has a script, and the script is well-formed.
    Scripted,
    /// Needs no script: no rules text at all, or text that is entirely a
    /// printed keyword the database already carries.
    NoScriptNeeded,
    /// Scripted, but [`validate_script`] finds the script does less than the
    /// printed text says.
    Malformed(Vec<Diagnostic>),
    /// Has rules text and no script. It will play — as a vanilla body, with its
    /// text silently doing nothing, which is the case worth warning about.
    Unscripted,
}

impl Cards {
    /// How completely this build implements one card.
    pub fn coverage(&self, def: CardDefId, card: &CardDef) -> Coverage {
        let script = self.script(def);
        if !script.is_vanilla() {
            let problems = validate_script(script);
            return if problems.is_empty() {
                Coverage::Scripted
            } else {
                Coverage::Malformed(problems)
            };
        }
        if card.effect.is_none() && card.trigger.is_none() {
            return Coverage::NoScriptNeeded;
        }
        if KEYWORD_ONLY.contains(&card.number.as_str()) {
            return Coverage::NoScriptNeeded;
        }
        Coverage::Unscripted
    }
}

impl op_deck::compat::CardSupport for Cards {
    fn support(&self, def: CardDefId, card: &CardDef) -> op_deck::compat::Support {
        use op_deck::compat::Support;
        match self.coverage(def, card) {
            Coverage::Scripted | Coverage::NoScriptNeeded => Support::Full,
            Coverage::Malformed(problems) => Support::Partial(
                problems
                    .iter()
                    .map(|d| d.to_string())
                    .collect::<Vec<_>>()
                    .join("; "),
            ),
            Coverage::Unscripted => Support::Unsupported("card script unavailable".into()),
        }
    }
}

/// Every script this crate provides.
pub fn all_scripts() -> Vec<(&'static str, CardScript)> {
    let mut out = Vec::new();
    out.extend(sets::st01::scripts());
    out.extend(sets::st02::scripts());
    out.extend(sets::st03::scripts());
    out.extend(sets::st04::scripts());
    out.extend(sets::st06::scripts());
    out.extend(sets::st08::scripts());
    out
}

/// Every problem [`op_core::validate`] finds across [`all_scripts`], tagged
/// with the card it came from and returned in script order.
///
/// A non-empty result is a bug, not a warning: each entry is a card that
/// compiles and then does less than its printed text says. It needs no
/// `CardDb`, so it runs on a bare clone.
pub fn validate_all_scripts() -> Vec<(String, Diagnostic)> {
    all_scripts()
        .into_iter()
        .flat_map(|(number, script)| {
            validate_script(&script)
                .into_iter()
                .map(move |d| (number.to_string(), d))
        })
        .collect()
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
    "ST03-008", // [Blocker]
    "ST04-011", // [Blocker]
    "ST06-007", // [Blocker]
];

#[cfg(test)]
mod tests {
    /// Two sets may not claim the same card number: `Cards::new` indexes by
    /// `CardDefId`, so the later entry would silently overwrite the earlier one.
    #[test]
    fn no_card_number_is_scripted_twice() {
        let mut seen = std::collections::BTreeSet::new();
        for (number, _) in super::all_scripts() {
            assert!(seen.insert(number), "{number} has two scripts");
        }
    }
}
