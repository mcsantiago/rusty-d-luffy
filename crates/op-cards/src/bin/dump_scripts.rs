//! Serialises every compiled card script to JSON.
//!
//! Scripts are built by Rust builders, which makes them easy to write and hard
//! to read as a whole. This dump gives a reviewable data form of the same
//! thing, and doubles as a working sample of the format a runtime loader would
//! have to accept — without committing to loading anything at run time.
//!
//!     cargo run -p op-cards --bin dump-scripts              # to stdout
//!     cargo run -p op-cards --bin dump-scripts -- out.json  # to a file
//!
//! Output is a card-number-keyed object in sorted order, so two runs of the
//! same tree produce byte-identical files and a diff means a script changed.

use std::collections::BTreeMap;
use std::process::ExitCode;

use op_cards::{all_scripts, validate_all_scripts};
use op_core::script::CardScript;

fn main() -> ExitCode {
    let scripts: BTreeMap<String, CardScript> = all_scripts()
        .into_iter()
        .map(|(number, script)| (number.to_string(), script))
        .collect();

    let json = match serde_json::to_string_pretty(&scripts) {
        Ok(json) => json,
        Err(err) => {
            eprintln!("serialising scripts: {err}");
            return ExitCode::FAILURE;
        }
    };

    match std::env::args().nth(1) {
        Some(path) => {
            if let Err(err) = std::fs::write(&path, format!("{json}\n")) {
                eprintln!("writing {path}: {err}");
                return ExitCode::FAILURE;
            }
            eprintln!("{} scripts written to {path}", scripts.len());
        }
        None => println!("{json}"),
    }

    // Reported, not fatal: the dump is still the most useful thing to look at
    // when a script is malformed. `every_script_is_well_formed` is what fails.
    for (number, diagnostic) in validate_all_scripts() {
        eprintln!("warning: {number} {diagnostic}");
    }

    ExitCode::SUCCESS
}
