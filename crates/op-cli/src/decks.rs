//! Built-in decklists.
//!
//! The two official starter products, as printed. Custom decklists can be
//! loaded from a file with `--deck`, one card number per line, the first being
//! the Leader.

use anyhow::{bail, Context, Result};
use op_core::DeckList;

pub fn by_name(name: &str) -> Option<DeckList> {
    match name.to_ascii_uppercase().as_str() {
        "ST01" | "ST-01" | "STRAWHAT" => Some(st01()),
        "ST02" | "ST-02" | "WORSTGEN" => Some(st02()),
        _ => None,
    }
}

pub const BUILTIN: &[&str] = &["ST01", "ST02"];

/// ST-01 Straw Hat Crew.
pub fn st01() -> DeckList {
    build(
        "ST01-001",
        &[
            ("ST01-002", 4), ("ST01-003", 4), ("ST01-004", 4), ("ST01-005", 2),
            ("ST01-006", 4), ("ST01-007", 4), ("ST01-008", 2), ("ST01-009", 4),
            ("ST01-010", 2), ("ST01-011", 4), ("ST01-012", 2), ("ST01-013", 4),
            ("ST01-014", 4), ("ST01-015", 2), ("ST01-016", 2), ("ST01-017", 2),
        ],
    )
}

/// ST-02 Worst Generation.
pub fn st02() -> DeckList {
    build(
        "ST02-001",
        &[
            ("ST02-002", 4), ("ST02-003", 4), ("ST02-004", 4), ("ST02-005", 4),
            ("ST02-006", 2), ("ST02-007", 4), ("ST02-008", 4), ("ST02-009", 2),
            ("ST02-010", 2), ("ST02-011", 4), ("ST02-012", 4), ("ST02-013", 2),
            ("ST02-014", 2), ("ST02-015", 4), ("ST02-016", 2), ("ST02-017", 2),
        ],
    )
}

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

/// Loads a decklist file: the Leader's card number first, then one card number
/// per line. Blank lines and `#` comments are ignored.
pub fn from_file(path: &str) -> Result<DeckList> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading decklist {path}"))?;
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
