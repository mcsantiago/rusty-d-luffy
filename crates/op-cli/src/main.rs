//! Play the One Piece Card Game against the AI in a terminal.
//!
//!     cargo run -p op-cli --release -- --help
//!
//! Card data is fetched automatically on first run.

mod render;

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use op_ai::{Agent, HeuristicAgent, IsmctsAgent, IsmctsConfig};
use op_cards::Cards;
use op_cli::{data, decks};
use op_core::card::CardDb;
use op_core::script::ScriptSource;
use op_core::view::PlayerView;
use op_core::{legal_actions, DeckList, Game, GameConfig, PlayerId, SessionLog};
use rand::rngs::StdRng;
use rand::SeedableRng;

struct Options {
    seed: u64,
    you: DeckList,
    opponent: DeckList,
    you_first: bool,
    difficulty: Difficulty,
    /// Run an AI-vs-AI game instead of prompting, for watching or profiling.
    autoplay: bool,
    /// Override for where the session log goes.
    log_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Difficulty {
    /// One-ply greedy.
    Easy,
    /// ISMCTS, modest budget.
    Normal,
    /// ISMCTS, large budget. Noticeably slower per decision.
    Hard,
}

impl Difficulty {
    fn agent(self, seed: u64) -> Box<dyn Agent> {
        match self {
            Difficulty::Easy => Box::new(HeuristicAgent::new(StdRng::seed_from_u64(seed))),
            Difficulty::Normal => Box::new(IsmctsAgent::new(IsmctsConfig {
                iterations: 300,
                rollout_depth: 50,
                seed,
                ..Default::default()
            })),
            Difficulty::Hard => Box::new(IsmctsAgent::new(IsmctsConfig {
                iterations: 1200,
                rollout_depth: 70,
                seed,
                ..Default::default()
            })),
        }
    }
}

fn main() -> Result<()> {
    let options = match parse_args()? {
        Some(options) => options,
        None => return Ok(()),
    };

    let db = data::load_db(&data::data_dir())?;
    let cards = Cards::new(&db);

    report_unscripted(&db, &cards, &options);

    let db = Arc::new(db);
    let scripts: Arc<dyn ScriptSource + Send + Sync> = Arc::new(cards);

    // The human always occupies P0 internally; `--second` swaps who begins.
    let config = GameConfig {
        seed: options.seed,
        first_player: if options.you_first {
            PlayerId::P0
        } else {
            PlayerId::P1
        },
        decks: [options.you.clone(), options.opponent.clone()],
        allow_illegal_decks: false,
    };
    // Cloned for the log, but written only once setup succeeds — a rejected
    // decklist should not leave a log behind for a game that never started.
    let logged = config.clone();
    let (mut game, opening) = Game::new(config, db, scripts)
        .map_err(|e| anyhow::anyhow!("could not start the game: {e}"))?;

    // Best-effort, exactly as in the desktop client: a session that cannot
    // write a log still plays. Autoplay runs are the ones this matters for —
    // they are long, unattended, and nobody is watching when they break.
    let mut debug = data::debug_dir(options.log_dir.as_deref()).and_then(|dir| {
        SessionLog::create(
            dir,
            &logged,
            Some(&op_ingest::source_ref()),
            vec![
                format!("difficulty={:?}", options.difficulty),
                format!("autoplay={}", options.autoplay),
                "client=cli".into(),
            ],
        )
        .map_err(|e| eprintln!("session log disabled: {e}"))
        .ok()
    });
    if let Some(log) = &debug {
        eprintln!("session log: {}", log.path().display());
    }
    if let Some(log) = &mut debug {
        log.record(None, &opening.events, &game.state, game.db());
    }

    let human = PlayerId::P0;
    let mut ai = options.difficulty.agent(options.seed ^ 0x5EED);
    let mut human_ai = options
        .autoplay
        .then(|| options.difficulty.agent(options.seed));

    let mut events = opening.events;
    let stdin = std::io::stdin();
    let mut input = stdin.lock();

    println!("\nOne Piece Card Game — seed {}\n", options.seed);

    loop {
        // Rendered from the human's projection, so the log physically cannot
        // name a card they are not entitled to see.
        for line in events
            .iter()
            .map(|e| e.project(&game.state, human))
            .filter_map(|e| render::event(&e, &game, human))
        {
            println!("  {line}");
        }

        if game.is_over() {
            break;
        }
        let Some(pending) = game.pending().cloned() else {
            bail!("engine parked with no decision pending");
        };

        let action = if pending.player() == human {
            match &mut human_ai {
                Some(agent) => agent.choose(&game, human),
                None => {
                    let view = PlayerView::project(&game.state, game.db(), &game.derived(), human);
                    println!("\n{}", render::board(&view, game.db()));
                    match prompt(&mut input, &game, &pending)? {
                        Some(action) => action,
                        // EOF: quit cleanly rather than looping on nothing.
                        None => return Ok(()),
                    }
                }
            }
        } else {
            let action = ai.choose(&game, pending.player());
            println!("  opponent: {}", render::action(&action, &game));
            action
        };

        let outcome = game
            .step(action.clone())
            .map_err(|e| anyhow::anyhow!("illegal action: {e}"))?;
        if let Some(log) = &mut debug {
            log.record(Some(&action), &outcome.events, &game.state, game.db());
        }
        events = outcome.events;
    }

    let view = PlayerView::project(&game.state, game.db(), &game.derived(), human);
    println!("\n{}", render::board(&view, game.db()));
    match game.result().and_then(|r| r.winner()) {
        Some(w) if w == human => println!("You win."),
        Some(_) => println!("You lose."),
        None => println!("Draw."),
    }
    Ok(())
}

/// Shows the numbered menu and reads a choice. `Ok(None)` means end of input.
fn prompt(
    input: &mut impl BufRead,
    game: &Game,
    pending: &op_core::Pending,
) -> Result<Option<op_core::Action>> {
    let legal = legal_actions(game);
    if legal.is_empty() {
        bail!("no legal action available at {pending:?}");
    }
    if legal.len() == 1 {
        println!("  (only one option: {})", render::action(&legal[0], game));
        return Ok(Some(legal[0].clone()));
    }

    println!("{}", question(pending));
    for (i, action) in legal.iter().enumerate() {
        println!("  {i:>2}) {}", render::action(action, game));
    }

    loop {
        print!("> ");
        std::io::stdout().flush().ok();
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim();
        if line.eq_ignore_ascii_case("q") || line.eq_ignore_ascii_case("quit") {
            return Ok(None);
        }
        match line.parse::<usize>() {
            Ok(i) if i < legal.len() => return Ok(Some(legal[i].clone())),
            _ => println!(
                "  enter a number from 0 to {}, or q to quit",
                legal.len() - 1
            ),
        }
    }
}

fn question(pending: &op_core::Pending) -> String {
    use op_core::Pending as P;
    match pending {
        P::Mulligan { .. } => "Keep your opening hand?".into(),
        P::MainAction { .. } => "Your main phase:".into(),
        P::Block { .. } => "You are being attacked — block?".into(),
        P::Counter { .. } => "Counter step:".into(),
        P::Trigger { .. } => "That life card has a [Trigger]:".into(),
        P::Choose { up_to, .. } => format!("Choose up to {up_to}:"),
    }
}

/// Warns about cards in the chosen decks that have no script, since they will
/// silently play as vanilla bodies.
fn report_unscripted(db: &CardDb, cards: &Cards, options: &Options) {
    let mut unscripted: Vec<&str> = Vec::new();
    for deck in [&options.you, &options.opponent] {
        for number in std::iter::once(&deck.leader).chain(deck.cards.iter()) {
            let Some(def) = db.by_number(number) else {
                continue;
            };
            let card = db.get(def);
            // A card with no rules text is correctly a vanilla body; only text
            // that goes unimplemented is worth warning about.
            let has_text = card.effect.is_some() || card.trigger.is_some();
            if has_text
                && cards.script(def).is_vanilla()
                && !op_cards::KEYWORD_ONLY.contains(&number.as_str())
                && !unscripted.contains(&card.number.as_str())
            {
                unscripted.push(&card.number);
            }
        }
    }
    if !unscripted.is_empty() {
        eprintln!(
            "warning: {} card(s) in these decks have no script and will play as \
             vanilla bodies: {}",
            unscripted.len(),
            unscripted.join(", ")
        );
    }
}

fn parse_args() -> Result<Option<Options>> {
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut you = op_cards::decks::st01();
    let mut opponent = op_cards::decks::st02();
    let mut you_first = true;
    let mut difficulty = Difficulty::Normal;
    let mut autoplay = false;
    let mut log_dir = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_help();
                return Ok(None);
            }
            "--seed" => {
                seed = args
                    .next()
                    .context("--seed needs a value")?
                    .parse()
                    .context("--seed must be a number")?;
            }
            "--deck" => you = resolve_deck(&args.next().context("--deck needs a value")?)?,
            "--opponent" => {
                opponent = resolve_deck(&args.next().context("--opponent needs a value")?)?
            }
            "--second" => you_first = false,
            "--easy" => difficulty = Difficulty::Easy,
            "--hard" => difficulty = Difficulty::Hard,
            "--autoplay" => autoplay = true,
            "--log" => log_dir = Some(PathBuf::from(args.next().context("--log needs a value")?)),
            other => bail!("unknown argument {other}; try --help"),
        }
    }

    Ok(Some(Options {
        seed,
        you,
        opponent,
        you_first,
        difficulty,
        autoplay,
        log_dir,
    }))
}

fn resolve_deck(spec: &str) -> Result<DeckList> {
    if let Some(deck) = decks::by_name(spec) {
        return Ok(deck);
    }
    decks::from_file(spec)
}

fn print_help() {
    println!(
        "onepiece — play the One Piece Card Game against an AI

USAGE:
    onepiece [OPTIONS]

OPTIONS:
    --deck <NAME|FILE>       your deck (default ST01)
    --opponent <NAME|FILE>   the AI's deck (default ST02)
    --second                 let the AI go first
    --easy                   one-ply greedy opponent
    --hard                   ISMCTS with a large search budget
    --autoplay               watch the AI play itself
    --seed <N>               fixed seed, for a reproducible game
    --log <DIR>              where to write this session's log
    -h, --help               show this help

Every session writes a replayable log to <data>/debug; set
OPSIM_DEBUG_DIR to move it, or to empty to turn it off. Check one
with:
    cargo run -p op-cli --bin op-replay -- <LOG>

Built-in decks: {}

A decklist file is one card number per line, the Leader first;
blank lines and # comments are ignored.

Card data must be fetched first:
    python3 tools/ingest/fetch_cards.py",
        decks::builtin_names().join(", ")
    );
}
