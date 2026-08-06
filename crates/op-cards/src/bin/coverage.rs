//! Reports which cards in `data/` have scripts.
//!
//! This is the gate on rolling a new set out: a set is playable when every
//! card in it is either scripted or deliberately vanilla.
//!
//!     cargo run -p op-cards --bin coverage

use std::collections::BTreeMap;
use std::process::ExitCode;

use op_cards::{Cards, INTENTIONALLY_VANILLA};
use op_core::card::CardDb;
use op_core::script::ScriptSource;

fn main() -> ExitCode {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/cards");

    let db = match CardDb::load_dir(&dir) {
        Ok(db) => db,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::FAILURE;
        }
    };
    let cards = Cards::new(&db);

    // Group by set prefix, e.g. "ST01".
    let mut by_set: BTreeMap<&str, (Vec<String>, Vec<String>, Vec<String>)> = BTreeMap::new();
    for (def, card) in db.iter() {
        let Some((set, _)) = card.number.split_once('-') else {
            continue; // the synthetic DON!! card
        };
        let entry = by_set.entry(set).or_default();
        if !cards.script(def).is_vanilla() {
            entry.0.push(card.number.clone());
        } else if INTENTIONALLY_VANILLA.contains(&card.number.as_str()) {
            entry.1.push(card.number.clone());
        } else {
            entry.2.push(card.number.clone());
        }
    }

    println!("card script coverage\n");
    let mut total_unimplemented = 0;
    for (set, (scripted, vanilla, unimplemented)) in &by_set {
        let total = scripted.len() + vanilla.len() + unimplemented.len();
        let done = scripted.len() + vanilla.len();
        let status = if unimplemented.is_empty() { "OK" } else { "--" };
        println!(
            "  {status} {set}  {done}/{total}  ({} scripted, {} vanilla)",
            scripted.len(),
            vanilla.len()
        );
        if !unimplemented.is_empty() {
            total_unimplemented += unimplemented.len();
            for number in unimplemented {
                let def = db.by_number(number).expect("number came from the db");
                let text = db
                    .get(def)
                    .effect
                    .as_deref()
                    .unwrap_or("(no text — add to INTENTIONALLY_VANILLA)");
                println!("       {number}  {text}");
            }
        }
    }

    if !cards.missing().is_empty() {
        println!("\nscripted but not present in data/: {:?}", cards.missing());
    }

    println!(
        "\n{} cards scripted across {} sets; {total_unimplemented} unimplemented",
        cards.scripted_count(),
        by_set.len()
    );

    if total_unimplemented > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
