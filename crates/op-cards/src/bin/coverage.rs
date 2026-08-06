//! Reports which cards in `data/` have scripts.
//!
//! This is the gate on rolling a new set out: a set is playable when every card
//! in it is either scripted or needs no script.
//!
//!     cargo run -p op-cards --bin coverage           # summary for every set
//!     cargo run -p op-cards --bin coverage -- OP01   # what OP01 still needs

use std::collections::BTreeMap;
use std::process::ExitCode;

use op_cards::{Cards, KEYWORD_ONLY};
use op_core::card::CardDb;
use op_core::script::ScriptSource;

#[derive(Default)]
struct SetStats {
    scripted: Vec<String>,
    /// Needs no script: no rules text, or text that is purely a printed keyword.
    no_script_needed: Vec<String>,
    /// Has text, has no script.
    todo: Vec<(String, String)>,
}

impl SetStats {
    fn total(&self) -> usize {
        self.scripted.len() + self.no_script_needed.len() + self.todo.len()
    }

    fn done(&self) -> usize {
        self.scripted.len() + self.no_script_needed.len()
    }
}

fn main() -> ExitCode {
    let only = std::env::args().nth(1).map(|s| s.to_uppercase());

    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/cards");
    let db = match CardDb::load_dir(&dir) {
        Ok(db) => db,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::FAILURE;
        }
    };
    let cards = Cards::new(&db);

    let mut by_set: BTreeMap<String, SetStats> = BTreeMap::new();
    for (def, card) in db.iter() {
        let Some((set, _)) = card.number.split_once('-') else {
            continue; // the synthetic DON!! card
        };
        let entry = by_set.entry(set.to_string()).or_default();

        if !cards.script(def).is_vanilla() {
            entry.scripted.push(card.number.clone());
        } else if card.effect.is_none() && card.trigger.is_none() {
            // No rules text: a vanilla body is the correct implementation.
            entry.no_script_needed.push(card.number.clone());
        } else if KEYWORD_ONLY.contains(&card.number.as_str()) {
            entry.no_script_needed.push(card.number.clone());
        } else {
            let text = card
                .effect
                .clone()
                .or_else(|| card.trigger.clone())
                .unwrap_or_default();
            entry.todo.push((card.number.clone(), text));
        }
    }

    // Detail for one set.
    if let Some(only) = &only {
        let Some(stats) = by_set.get(only) else {
            eprintln!("no set {only} in data/; known: {:?}", by_set.keys());
            return ExitCode::FAILURE;
        };
        println!(
            "{only}: {}/{} done, {} to script\n",
            stats.done(),
            stats.total(),
            stats.todo.len()
        );
        for (number, text) in &stats.todo {
            println!("  {number}");
            for line in text.split("<br>") {
                println!("      {}", line.trim());
            }
        }
        return ExitCode::SUCCESS;
    }

    // Summary for everything.
    println!("card script coverage\n");
    let (mut complete, mut todo_total) = (0, 0);
    for (set, stats) in &by_set {
        let ok = stats.todo.is_empty();
        if ok {
            complete += 1;
        }
        todo_total += stats.todo.len();
        println!(
            "  {} {set:6} {:>4}/{:<4} ({} scripted, {} need none, {} to script)",
            if ok { "OK" } else { "--" },
            stats.done(),
            stats.total(),
            stats.scripted.len(),
            stats.no_script_needed.len(),
            stats.todo.len(),
        );
    }

    if !cards.missing().is_empty() {
        println!("\nscripted but not present in data/: {:?}", cards.missing());
    }

    println!(
        "\n{}/{} sets complete; {} cards scripted, {todo_total} still to script",
        complete,
        by_set.len(),
        cards.scripted_count(),
    );
    if todo_total > 0 {
        println!("run with a set name for detail, e.g. `-- {}`",
            by_set.iter().find(|(_, s)| !s.todo.is_empty()).map(|(k, _)| k.as_str()).unwrap_or("OP01"));
    }

    if todo_total > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
