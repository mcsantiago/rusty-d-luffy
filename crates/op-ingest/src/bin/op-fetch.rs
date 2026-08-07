//! Fetches card data from the command line.
//!
//! The same fetch the clients run on first launch, exposed for scripting and
//! for CI — which needs card data to run the integration tests but has no
//! reason to pull several hundred megabytes of art.
//!
//!     op-fetch --data-dir data --packs ST-01 ST-02
//!     op-fetch --data-dir data --all --images

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut data_dir: Option<PathBuf> = None;
    let mut packs: Vec<String> = Vec::new();
    let mut plan = op_ingest::Plan {
        packs: Vec::new(),
        images: false,
        refresh: false,
        jobs: 4,
    };
    let mut all = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_help();
                return ExitCode::SUCCESS;
            }
            "--data-dir" => match args.next() {
                Some(dir) => data_dir = Some(PathBuf::from(dir)),
                None => return fail("--data-dir needs a value"),
            },
            "--all" => all = true,
            "--images" => plan.images = true,
            "--refresh" => plan.refresh = true,
            "--jobs" => match args.next().and_then(|j| j.parse().ok()) {
                Some(jobs) => plan.jobs = jobs,
                None => return fail("--jobs needs a number"),
            },
            "--packs" => {
                // Consume until the next flag.
                for arg in args.by_ref() {
                    if arg.starts_with("--") {
                        return fail("--packs must come last");
                    }
                    packs.push(arg);
                }
            }
            other => return fail(&format!("unknown argument {other}")),
        }
    }

    if !all && packs.is_empty() {
        packs = vec!["ST-01".into(), "ST-02".into()];
    }
    plan.packs = if all { Vec::new() } else { packs };

    let dir = data_dir.unwrap_or_else(|| op_ingest::default_data_dir("dev.onepiecesim.desktop"));
    println!("fetching into {}", dir.display());

    let report = |p: op_ingest::Progress| {
        if let op_ingest::Progress::Message(m) = p {
            println!("  {m}");
        }
    };

    match op_ingest::run(&dir, &plan, &report) {
        Ok(summary) => {
            println!(
                "done: {} cards across {} product(s), {} image(s)",
                summary.cards, summary.products, summary.images_fetched
            );
            if summary.failed.is_empty() {
                ExitCode::SUCCESS
            } else {
                eprintln!("failed: {}", summary.failed.join(", "));
                ExitCode::FAILURE
            }
        }
        Err(err) => fail(&err.to_string()),
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("{message}");
    ExitCode::FAILURE
}

fn print_help() {
    println!(
        "op-fetch — fetch One Piece Card Game data

USAGE:
    op-fetch [OPTIONS] [--packs NAME...]

OPTIONS:
    --data-dir <DIR>   where to write (default: per-user app data directory)
    --all              every pack
    --images           also cache card art (~750 MB for --all)
    --refresh          re-download packs already present
    --jobs <N>         parallel downloads (default 4)
    --packs NAME...    packs to fetch; must come last

Names ignore case and dashes: ST-01, st01 and \"ST 01\" are the same pack."
    );
}
