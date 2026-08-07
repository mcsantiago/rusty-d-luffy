//! Replay a session log and check it against what was recorded.
//!
//!     cargo run -p op-cli --bin op-replay -- <LOG>...
//!
//! A log records the config, the seed and every action, so it is a complete
//! reproducer. This rebuilds the game from one and steps the recorded actions
//! back through the engine, comparing the state hash and the emitted events at
//! every step. The first step that differs is the answer: it names the action
//! whose behaviour changed.
//!
//! That makes any log kept from a past release a regression test. Replaying an
//! old session after a rules change either matches or names the step where the
//! rules moved — which is a much cheaper bisect than reproducing the bug.
//!
//! Logs are omniscient (they record `GameEvent`, so they hold both hands), so
//! this is a local debugging tool. Do not wire it into anything a player can
//! reach mid-game.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use op_cards::Cards;
use op_cli::data;
use op_core::replay::{self, Divergence};
use op_core::script::ScriptSource;

struct Options {
    logs: Vec<PathBuf>,
    data_dir: Option<PathBuf>,
    /// Print every step's hash, not just the verdict.
    verbose: bool,
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        // A divergence is a finding, not a crash: report it and exit non-zero
        // so this is usable from a script.
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<bool> {
    let Some(options) = parse_args()? else {
        return Ok(true);
    };

    let data_dir = options.data_dir.clone().unwrap_or_else(data::data_dir);
    let db = data::load_db(&data_dir)?;
    let scripts: Arc<dyn ScriptSource + Send + Sync> = Arc::new(Cards::new(&db));
    let db = Arc::new(db);

    let build_ref = op_ingest::source_ref();
    let mut all_matched = true;

    for path in &options.logs {
        println!("{}", path.display());
        let record = replay::read(path).with_context(|| format!("reading {}", path.display()))?;

        let header = &record.header;
        println!(
            "  seed {}, {} first, {} vs {}, {} steps",
            header.seed,
            seat(header.first_player),
            header.decks[0].leader,
            header.decks[1].leader,
            record.steps.len()
        );
        if !header.notes.is_empty() {
            println!("  notes: {}", header.notes.join(", "));
        }
        // The reason the ref is in the header at all: without this line, a
        // divergence caused by a bumped pin looks exactly like an engine bug.
        match header.card_data_ref.as_deref() {
            Some(logged) if logged == build_ref => {}
            Some(logged) => println!(
                "  warning: recorded against card data {logged}, this build uses \
                 {build_ref} — a divergence below may be the data, not the engine"
            ),
            None => println!("  warning: no card data revision recorded"),
        }
        if record.truncated {
            println!("  note: the log ends mid-record; the session was killed while writing");
        }

        if options.verbose {
            for step in &record.steps {
                let action = step
                    .action
                    .as_ref()
                    .map(|a| format!("{a:?}"))
                    .unwrap_or_else(|| "<setup>".into());
                println!("    {:>4}  {:#018x}  {action}", step.n, step.state_hash);
            }
        }

        match record.verify(Arc::clone(&db), Arc::clone(&scripts)) {
            Ok(verified) => println!(
                "  OK — {} steps replayed, final hash {:#018x}",
                verified.steps, verified.final_hash
            ),
            Err(divergence) => {
                all_matched = false;
                report(&record, &divergence);
            }
        }
    }

    Ok(all_matched)
}

/// Prints a divergence with the recorded step's resolved card names.
///
/// The ids in an action mean nothing on their own, which is why the writer
/// resolves them; reproducing that here is the difference between "step 87
/// diverged" and a report someone can act on.
fn report(record: &replay::SessionRecord, divergence: &Divergence) {
    println!("  DIVERGED — {divergence}");

    let step = match divergence {
        Divergence::Rejected { step, .. }
        | Divergence::Hash { step, .. }
        | Divergence::Events { step, .. }
        | Divergence::MissingAction { step } => Some(*step),
        Divergence::Setup(_) => None,
    };
    let Some(step) = step.and_then(|n| record.steps.iter().find(|s| s.n == n)) else {
        return;
    };

    println!("    turn {}, {}", step.turn, step.phase);
    if let Some(action) = &step.action {
        println!("    recorded action: {action:?}");
    }
    for card in &step.cards {
        println!(
            "    card {} = {} {}",
            card.instance, card.definition, card.name
        );
    }
    if let Divergence::Events {
        expected, actual, ..
    } = divergence
    {
        println!("    recorded events: {expected:?}");
        println!("    replayed events: {actual:?}");
    }
}

fn seat(player: u8) -> &'static str {
    if player == 0 {
        "P0"
    } else {
        "P1"
    }
}

fn parse_args() -> Result<Option<Options>> {
    let mut logs = Vec::new();
    let mut data_dir = None;
    let mut verbose = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_help();
                return Ok(None);
            }
            "--data-dir" => {
                data_dir = Some(PathBuf::from(
                    args.next().context("--data-dir needs a value")?,
                ))
            }
            "-v" | "--verbose" => verbose = true,
            other if other.starts_with('-') => bail!("unknown argument {other}; try --help"),
            other => logs.push(PathBuf::from(other)),
        }
    }

    if logs.is_empty() {
        print_help();
        bail!("no log given");
    }
    Ok(Some(Options {
        logs,
        data_dir,
        verbose,
    }))
}

fn print_help() {
    println!(
        "op-replay — replay a session log and check it against what was recorded

USAGE:
    op-replay [OPTIONS] <LOG>...

OPTIONS:
    --data-dir <DIR>   where card data lives (default: the client's)
    -v, --verbose      print every step's hash
    -h, --help         show this help

Exits non-zero if any log fails to replay. Logs are written to
<data>/debug by both clients; OPSIM_DEBUG_DIR moves or disables them."
    );
}
