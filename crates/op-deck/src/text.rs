//! The OPTCGSim decklist text format.
//!
//! There is no specification for this format — it is whatever OPTCGSim exports
//! and every other tool learned to read. In practice that is `<quantity>
//! <card number>` per line, with `4x ST01-002` and `4xST01-002` both in
//! circulation. Parsing is therefore liberal, and anything unrecognised is
//! *reported* rather than skipped: a line silently dropped from a decklist
//! produces a 47-card deck and no explanation of where the other three went.
//!
//! Parsing knows nothing about the card database. A number that looks
//! well-formed here may still name no card, which is [`crate::resolve`]'s
//! problem, and a deck that parses cleanly may still be illegal, which is
//! [`crate::legality`]'s. Keeping the three apart is what lets the UI say which
//! of them a deck is failing.
//!
//! [`write`] is the same format in the other direction, so a deck built here
//! can be pasted into any tool that reads one.

use crate::DeckEntry;

/// A problem with one line of input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseProblem {
    /// 1-based, so it matches what a text editor shows.
    pub line: usize,
    /// The line as written, trimmed. Echoed back so the message can quote it.
    pub text: String,
    pub kind: ParseProblemKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseProblemKind {
    /// Not `<quantity> <card number>`, and not a bare card number either.
    Unrecognised,
    /// A quantity that is not a positive whole number.
    ///
    /// A quantity *above* the 4-copy limit is not a parse problem: 5-1-2-3 is a
    /// deck construction rule, and reporting it here would couple the format to
    /// the rules. It surfaces as [`crate::legality::LegalityError::TooManyCopies`].
    Quantity { found: String },
}

impl std::fmt::Display for ParseProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.kind {
            ParseProblemKind::Unrecognised => {
                write!(f, "line {}: cannot read {:?}", self.line, self.text)
            }
            ParseProblemKind::Quantity { found } => write!(
                f,
                "line {}: invalid quantity {found:?} in {:?}",
                self.line, self.text
            ),
        }
    }
}

/// The result of reading a decklist: what was understood, and what was not.
///
/// Both halves are always returned. A list with one bad line still yields the
/// other 49 entries, so the UI can show the deck taking shape next to the
/// errors instead of refusing the paste outright.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Parsed {
    pub entries: Vec<DeckEntry>,
    pub problems: Vec<ParseProblem>,
}

/// Largest quantity a single line may name.
///
/// Well above the 4-copy limit, which is deliberately not enforced here, but
/// low enough that a card number misread as a count cannot ask for a million
/// copies.
const MAX_QUANTITY: u32 = 99;

/// Reads an OPTCGSim-style decklist.
///
/// Blank lines and `#` comments are ignored, matching the decklist files the
/// terminal client already loads. A line that is a bare card number counts as
/// one copy, which makes those same files importable unchanged — a card number
/// always contains a `-`, so it can never be mistaken for a quantity.
pub fn parse(text: &str) -> Parsed {
    let mut parsed = Parsed::default();

    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        // Clipboard text from a Windows tool arrives with a BOM on line 1 and
        // CRLF throughout; neither should read as part of a card number.
        let text = raw.trim_start_matches('\u{feff}');
        let text = text.split('#').next().unwrap_or("").trim();
        if text.is_empty() {
            continue;
        }

        match read_line(text) {
            Ok((quantity, number)) => parsed.add(number, quantity),
            Err(kind) => parsed.problems.push(ParseProblem {
                line,
                text: text.to_string(),
                kind,
            }),
        }
    }

    parsed
}

impl Parsed {
    /// Adds `quantity` copies of `number`, merging with an existing entry.
    ///
    /// Merging rather than appending because a hand-edited list may name the
    /// same card twice, and two entries for one number would then defeat the
    /// 4-copy check by splitting 5 into 3 and 2. The first mention keeps its
    /// position: entry order survives into the decklist, where it decides
    /// instance id assignment and therefore the game a seed produces.
    fn add(&mut self, number: String, quantity: u32) {
        if let Some(existing) = self.entries.iter_mut().find(|e| e.number == number) {
            existing.quantity = existing.quantity.saturating_add(quantity);
        } else {
            self.entries.push(DeckEntry { number, quantity });
        }
    }
}

/// Splits one line into a quantity and a card number.
fn read_line(text: &str) -> Result<(u32, String), ParseProblemKind> {
    // `4 ST01-002`, `4x ST01-002` and `4xST01-002` all split here; a bare
    // number has no split point and falls through as a single copy.
    let (count, number) = match text.split_once(|c: char| c.is_whitespace()) {
        Some((head, tail)) => (head, tail.trim()),
        None => match split_x_prefix(text) {
            Some(split) => split,
            None if is_card_number(text) => return Ok((1, normalise(text))),
            None => return Err(ParseProblemKind::Unrecognised),
        },
    };

    // Only the count can carry the `x`; `split_x_prefix` has already removed it
    // from the number, and stripping a leading `x` there would corrupt any
    // future set whose letters begin with one.
    let count = count.trim().trim_end_matches(['x', 'X']);

    if !is_card_number(number) {
        return Err(ParseProblemKind::Unrecognised);
    }
    let quantity: u32 = count.parse().map_err(|_| ParseProblemKind::Quantity {
        found: count.into(),
    })?;
    if quantity == 0 || quantity > MAX_QUANTITY {
        return Err(ParseProblemKind::Quantity {
            found: count.into(),
        });
    }

    Ok((quantity, normalise(number)))
}

/// Splits `4xST01-002` into its count and card number.
///
/// Only at an `x` that follows digits, so the `X` inside a card number of some
/// future set cannot be mistaken for the separator.
fn split_x_prefix(text: &str) -> Option<(&str, &str)> {
    let at = text.find(['x', 'X'])?;
    let (count, rest) = text.split_at(at);
    if count.is_empty() || !count.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((count, &rest[1..]))
}

/// Whether `text` is shaped like a card number, e.g. `ST01-002` or `P-001`.
///
/// Deliberately structural rather than a set membership test: whether the card
/// *exists* is [`crate::resolve`]'s question, and a database fetched for two
/// packs must still be able to tell "you have not downloaded OP12" from "that
/// is not a card number".
fn is_card_number(text: &str) -> bool {
    text.contains('-')
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Normalises a card number to the form the database keys on.
///
/// Uppercased, because card numbers are uppercase in the data and a decklist
/// may not be. Alternate printings are folded onto their base number:
/// `OP01-016_p1` is parallel art of `OP01-016`, the same card by 2-14-3 and the
/// same card for the 4-copy limit of 5-1-2-3, and `CardDb` drops variants at
/// load for exactly that reason.
///
/// Folding has to happen here rather than at resolution, because [`parse`]
/// merges entries by number: left as two entries, four of each printing would
/// pass the copy limit separately and field eight.
fn normalise(number: &str) -> String {
    let number = number.trim();
    // `op_core` owns what counts as an alternate printing; follow its answer
    // rather than re-deciding here what a `_` means.
    let base = if op_core::card::is_art_variant(number) {
        number.split_once('_').map_or(number, |(base, _)| base)
    } else {
        number
    };
    base.to_ascii_uppercase()
}

/// Writes a decklist in the format [`parse`] reads.
///
/// The Leader comes first, as every tool in the ecosystem writes it, and the
/// remaining entries keep the order they are given — which is the order that
/// reaches `DeckList` and decides instance ids, so an exported deck reimports
/// as the same deck rather than merely an equivalent one.
pub fn write(leader: &str, entries: &[DeckEntry]) -> String {
    let mut out = format!("1 {leader}\n");
    for entry in entries {
        out.push_str(&format!("{} {}\n", entry.quantity, entry.number));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(text: &str) -> Vec<(String, u32)> {
        parse(text)
            .entries
            .into_iter()
            .map(|e| (e.number, e.quantity))
            .collect()
    }

    #[test]
    fn reads_the_quantity_and_card_number_forms_in_circulation() {
        let text = "1 ST01-001\n4x ST01-002\n2xST01-003\nST01-004";
        assert_eq!(
            entries(text),
            [
                ("ST01-001".to_string(), 1),
                ("ST01-002".to_string(), 4),
                ("ST01-003".to_string(), 2),
                ("ST01-004".to_string(), 1),
            ]
        );
    }

    #[test]
    fn blank_lines_and_comments_are_ignored() {
        let text = "\n# my deck\n1 ST01-001   # the leader\n\n  \n4 ST01-002\n";
        assert_eq!(
            entries(text),
            [("ST01-001".to_string(), 1), ("ST01-002".to_string(), 4)]
        );
        assert!(parse(text).problems.is_empty());
    }

    #[test]
    fn clipboard_text_from_windows_survives_its_bom_and_line_endings() {
        let text = "\u{feff}1 ST01-001\r\n4 ST01-002\r\n";
        assert_eq!(
            entries(text),
            [("ST01-001".to_string(), 1), ("ST01-002".to_string(), 4)]
        );
    }

    #[test]
    fn lowercase_card_numbers_are_accepted() {
        assert_eq!(entries("4 st01-002"), [("ST01-002".to_string(), 4)]);
    }

    /// Splitting one card number across two lines would otherwise let 5 copies
    /// pass the 4-copy check as a 3 and a 2.
    #[test]
    fn repeated_card_numbers_merge_into_one_entry() {
        assert_eq!(entries("3 ST01-002\n2 ST01-002"), [("ST01-002".into(), 5)]);
    }

    /// Alternate printings are the same card for deck construction (5-1-2-3),
    /// and `CardDb` drops them at load. Kept as separate entries they would
    /// pass the 4-copy check separately and field eight.
    #[test]
    fn alternate_printings_fold_onto_the_card_they_are_a_printing_of() {
        assert_eq!(
            entries("4 OP01-016\n1 OP01-016_p1"),
            [("OP01-016".to_string(), 5)]
        );
        assert_eq!(entries("2 EB01-006_r1"), [("EB01-006".to_string(), 2)]);
    }

    /// The first mention fixes the position: entry order reaches `DeckList`,
    /// where it decides instance ids and so the game a seed produces.
    #[test]
    fn a_merged_entry_keeps_its_first_position() {
        assert_eq!(
            entries("1 ST01-005\n4 ST01-002\n1 ST01-005"),
            [("ST01-005".to_string(), 2), ("ST01-002".to_string(), 4)]
        );
    }

    #[test]
    fn a_quantity_that_is_not_a_positive_number_is_reported() {
        for bad in ["0 ST01-002", "-1 ST01-002", "many ST01-002", "100 ST01-002"] {
            let problems = parse(bad).problems;
            assert!(
                matches!(problems.as_slice(), [p] if matches!(p.kind, ParseProblemKind::Quantity { .. })),
                "{bad} should report an invalid quantity, got {problems:?}"
            );
        }
    }

    /// 5-1-2-3 belongs to the legality pass. A five-copy line parses, so the
    /// error the user sees names the rule rather than the file format.
    #[test]
    fn a_quantity_over_the_copy_limit_still_parses() {
        assert_eq!(entries("5 ST01-002"), [("ST01-002".to_string(), 5)]);
    }

    #[test]
    fn an_unreadable_line_is_reported_and_the_rest_still_parse() {
        let parsed = parse("1 ST01-001\nwhat is this\n4 ST01-002");
        assert_eq!(parsed.entries.len(), 2);
        assert_eq!(
            parsed.problems,
            [ParseProblem {
                line: 2,
                text: "what is this".into(),
                kind: ParseProblemKind::Unrecognised,
            }]
        );
    }

    #[test]
    fn problems_carry_the_line_number_the_editor_shows() {
        let parsed = parse("# header\n\n1 ST01-001\nnonsense");
        assert_eq!(parsed.problems[0].line, 4);
    }

    /// Export and import are the same format, and the round trip preserves
    /// entry order — not merely the multiset — because that order decides
    /// instance ids at setup.
    #[test]
    fn a_written_deck_reads_back_unchanged() {
        let deck = [
            DeckEntry {
                number: "ST01-005".into(),
                quantity: 2,
            },
            DeckEntry {
                number: "ST01-002".into(),
                quantity: 4,
            },
            DeckEntry {
                number: "ST01-003".into(),
                quantity: 1,
            },
        ];
        let parsed = parse(&write("ST01-001", &deck));
        assert!(parsed.problems.is_empty());
        assert_eq!(parsed.entries[0].number, "ST01-001");
        assert_eq!(parsed.entries[1..], deck);
    }
}
