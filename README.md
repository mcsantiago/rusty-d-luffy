# OnePieceSim

A deterministic, event-driven rules engine for the **One Piece Card Game**
(Bandai OPTCG), built to serve three consumers off one kernel: local play against
an AI, a reinforcement-learning sandbox, and online multiplayer.

Rules are implemented against the official **Comprehensive Rules v1.2.0**
(2026-01-16). Non-obvious code carries the rule number it comes from, and the
test suite is named for the clauses it pins down.

## Quick start

```bash
# Desktop app — fetches card data on first run, no setup needed
cargo run -p op-desktop --release

# Terminal client — needs the data fetched first
python3 tools/ingest/fetch_cards.py --packs ST-01 ST-02 --images
cargo run -p op-cli --release -- --help
cargo run -p op-cli --release            # you (ST-01) vs AI (ST-02)
cargo run -p op-cli --release -- --hard --second
cargo run -p op-cli --release -- --autoplay --seed 7   # watch the AI play itself

# 3. Verify
cargo test
cargo run -p op-cards --bin coverage     # which cards have scripts
```

## Layout

| Crate | What it is |
|---|---|
| `op-core` | The kernel: zones, state, turn and battle machines, effect resolution, derived characteristics, legal-action generation, per-player views |
| `op-cards` | Card scripts, one module per product, plus the scripting DSL |
| `op-ai` | Determinization, a heuristic evaluation/agent, and ISMCTS |
| `op-cli` | Terminal client |
| `op-desktop` | Tauri desktop client; `client/` holds the front end |
| `tools/ingest` | Fetches card data into `data/` |

## Design

**Determinism.** A game is a pure function of `(GameConfig, seed, [Action])`.
All randomness runs through one seeded `ChaCha8Rng`; rules paths use ordered
containers and dense integer ids only; there is no floating point in the rules.
A replay is a seed plus an action list.

**Effect resolution suspends as data.** The ruleset has no MTG-style stack —
effects resolve immediately, turn player first (8-6-1) — but they routinely stop
mid-way to ask a player something: choose a target (8-4-4), activate a
`[Trigger]` in the middle of damage (8-6-2-1), pay a cost (8-4-1-3). That
suspension is an instruction pointer in a plain-data `EffectFrame`, not a
coroutine, so `GameState` stays `Clone + Serialize` even mid-effect. MCTS
cloning, server snapshots, and save/replay all depend on that.

**Characteristics are derived, never stored.** Power, cost and keywords are
recomputed from printed values plus DON!! attachment plus modifiers plus
permanent effects, the last applied in the fixpoint loop of 8-1-3-3-5. This is
the one decision that cannot be retrofitted.

**Hidden information is a type boundary.** `GameState` is omniscient and never
leaves the server; clients and imperfect-information agents get `PlayerView`.
Agents that must not cheat destroy what they cannot see via
`op_ai::determinize`, which reshuffles hidden cards among the hidden slots they
could occupy.

**One legal-action generator.** `op_core::legal_actions` is the move generator
for search, the RL action mask, and the server's validator — so the three cannot
disagree about what is legal.

## Desktop client

Tauri rather than Electron because the engine is already Rust: `op-core` links
straight into the app binary, so a UI click calls `Game::step` in-process. No
sidecar, no hand-written IPC protocol, no bundled Chromium. The front end under
`client/` is plain ES modules — there is no JavaScript build step.

The UI is a **renderer**. It is handed a `PlayerView`, a log of already-projected
`PlayerEvent`s, and the legal actions for the human seat — never `GameState`.
That is the same boundary the multiplayer server will use, so this front end
should survive the transport becoming a socket.

**First run fetches its own data.** `data/` is empty on a fresh clone — the card
text and art are Bandai's copyright and are not vendored — so the window opens
first and the app shells out to `tools/ingest/fetch_cards.py` on a worker
thread, streaming progress into the setup panel. Only *Start game* is gated
while that runs; the window stays live. Shelling out rather than reimplementing
the fetch keeps one copy of the fiddly parts (pack aliasing, alternate-printing
filtering, retries), at the cost of needing `python3` on PATH — which is
reported plainly if missing.

Everything is fetched up front, the way a phone TCG client does it: all 59
sets, card data and art, ~2,700 files and roughly 750 MB. It takes a few
minutes once, and afterwards the app is entirely offline.

Two things make that survivable. The fetch **skips what is already on disk**, so
an interrupted run resumes instead of restarting. And startup checks for missing
*art*, not just missing card data — an interrupted run leaves cards complete and
art partial, and checking only for card JSON would call that finished.

*Start game* stays disabled until the download completes. Art arrives across all
59 sets in no particular order, so a game started early would render text
placeholders rather than the cards in your own deck. A complete install skips
the fetch entirely and opens offline.

Card art is served as data URIs. Without it the UI falls back to drawing text
cards, so the app still runs.

> `client/` is embedded into the binary at compile time, so **editing the front
> end requires `cargo build -p op-desktop`** — a browser refresh will not pick
> changes up.

## Debug logs

Every session writes `debug/session-<time>-<seed>.jsonl` (gitignored). JSON
Lines, flushed per step, so a log from a run that crashed still has its tail.

Because a game is a pure function of `(config, seed, [Action])`, the log is a
**reproducer** rather than a trace: the header carries the seed and both
decklists, and each step carries the action, the resulting `GameEvent`s, and the
state hash. Replaying the recorded actions into a fresh game from the same seed
must reproduce those hashes — a divergence shows up there before it shows up as
wrong behaviour.

The log is **omniscient**: it records `GameEvent`, not `PlayerEvent`, so it
contains both hands. That is what makes it useful for diagnosing the engine, and
why it must never be surfaced to a player mid-game.

`OPSIM_DEBUG_DIR=` disables logging; any other value overrides the directory.

## Card data

Fetched from [`buhbbl/punk-records`](https://github.com/buhbbl/punk-records)
(static JSON generated by [`Coko7/vegapull`](https://github.com/Coko7/vegapull)
from Bandai's official site) into `data/`, which is gitignored. Card text and
images are Bandai's copyright; nothing is vendored into this repo or its
binaries.

```bash
python3 tools/ingest/fetch_cards.py                   # ST-01 and ST-02
python3 tools/ingest/fetch_cards.py --packs OP-01 EB-04 PROMO
python3 tools/ingest/fetch_cards.py --all             # 2,665 cards, ~1s
python3 tools/ingest/fetch_cards.py --list            # available pack names
```

Names are matched case- and dash-insensitively, so `OP-01`, `op01` and `OP 01`
are the same pack. Already-fetched packs are skipped, so an interrupted run
resumes; `--refresh` forces a re-download and `--jobs 1` serialises if a network
objects.

One request per product, from upstream's `english/data/<pack_id>.json`
aggregates. The GitHub API is not used at all, so the 60-requests-per-hour
unauthenticated limit never applies.

### Upstream quirks the ingest handles

- **A Leader's Life value is stored in the `cost` field.** Mapped in
  `card.rs`; pinned by a test against real data.
- **"No rules text" is a literal `"-"`,** not null or an empty string — 317
  cards across the pool. Normalised to `None` in `card.rs`, so vanilla cards
  aren't mistaken for unimplemented ones.
- **Alternate printings get their own ids** — `OP01-016_p1` (parallel),
  `EB01-006_r1`. They are the *same card*, including for the four-copy deck
  limit (5-1-2-3), so registering them separately would let a deck field eight.
  They are skipped on fetch and dropped again at load
  (`op_core::card::is_art_variant`); every variant has a base card, so this is
  lossless. Of upstream's 4,672 entries, **2,665 are distinct cards** and 2,007
  are alternate art.
- **Not every product code appears as a label.** OP-14 and OP-15 ship as
  combined products labelled `OP14-EB04` / `OP15-EB04`, and the promo and
  "Other Product" packs carry no label at all. The ingest registers each
  component as an alias, so `--packs OP-14` and `--packs PROMO` resolve;
  `EB-04` correctly resolves to both boosters that carry it.

### Coverage of the source

Complete, with no gaps in card numbering, for **OP-01 → OP-16, EB-01 → EB-04,
ST-01 → ST-36**. Two known holes: promos are partial (~105 of ~155 `P-xxx`
numbers), and `PRB-01` has a single entry. Neither affects the starter-deck
pool this project currently scripts.

Card *scripts* — our encoding of what each card does — are compiled in and keyed
by card number.

A card with no script plays as a vanilla body, which is correct for the many
cards with no text. The coverage report separates the two:

```bash
cargo run -p op-cards --bin coverage          # one line per set
cargo run -p op-cards --bin coverage -- EB01  # what EB01 still needs, with text
```

## Adding a set

1. `python3 tools/ingest/fetch_cards.py --packs OP-01`
2. `cargo run -p op-cards --bin coverage -- OP01` to list what needs scripting
3. Add `crates/op-cards/src/sets/op01.rs`, register it in `sets/mod.rs` and
   `all_scripts()`, until the set reports `OK`
4. Extend the DSL vocabulary in `op-cards/src/dsl.rs` (and the ops in
   `op-core/src/effect.rs`) where existing pieces do not fit

## Status

Implemented: the rules kernel, ST-01 and ST-02 at full script coverage, the
legal-action generator, heuristic and ISMCTS agents, and the terminal client.

Not yet built: PyO3 bindings and the Gymnasium environment, and the authoritative
multiplayer server and web client. Both sit on top of the existing kernel — the
turn-based ruleset means lockstep on actions is sufficient for netcode, with no
rollback needed.

### Known simplifications

These are deliberate and localised; each is commented at its site.

- Playing into a full Character area is rejected rather than offering the
  trash-to-make-room choice of 3-7-6-1.
- `DigTop` returns unchosen cards to the bottom of the deck in draw order; the
  card text permits any order, but the ordering is unobservable and enumerating
  permutations would inflate the action space for search and RL.
- Activation costs that trash from hand take the leftmost cards rather than
  prompting. Only ST02-001 uses this, for a single card.
- An unscripted `[Trigger]` is treated as absent rather than offering a choice
  that would do nothing.
