//! Decklists for the terminal client.
//!
//! The built-in lists live in [`op_cards::decks`] so that every client and test
//! suite sees the same set; this module re-exports them and adds the file
//! loader, which is the one thing only the CLI needs. Custom decklists load
//! with `--deck`, one card number per line, the first being the Leader.

use anyhow::{bail, Context, Result};
use op_core::DeckList;

pub use op_cards::decks::{by_name, ALL};

/// Deck names for `--help` and for the "unknown deck" message.
pub fn builtin_names() -> Vec<&'static str> {
    ALL.iter().map(|d| d.id).collect()
}

/// Loads a decklist file: the Leader's card number first, then one card number
/// per line. Blank lines and `#` comments are ignored.
pub fn from_file(path: &str) -> Result<DeckList> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading decklist {path}"))?;
    let mut numbers = text
        .lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty());

    let Some(leader) = numbers.next() else {
        bail!("{path} is empty; the first line must be the Leader's card number");
    };
    Ok(DeckList {
        leader: leader.to_string(),
        cards: numbers.map(|s| s.to_string()).collect(),
    })
}
