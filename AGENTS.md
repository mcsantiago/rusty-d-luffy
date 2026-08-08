# Working in this repository

Guidance for anyone — human or agent — making changes here. It covers the
things that are not obvious from reading a single file, and the invariants that
are easy to break without the tests noticing.

## Commands

```bash
cargo test --workspace --release   # release: the ISMCTS matches are ~60s even
                                   # fanned out across cores, and far worse in debug
cargo fmt --all
cargo clippy --workspace --all-targets   # CI runs this with -D warnings

cargo run -p op-desktop            # desktop client
cargo run -p op-cli -- --help      # terminal client
cargo run -p op-cards --bin coverage        # which cards have scripts
cargo run -p op-cards --bin coverage -- ST03  # what one set still needs
cargo run -p op-cards --bin dump-scripts    # every script as JSON

python3 tools/rules/fetch_rules.py         # Comprehensive Rules into data/rules/
cargo run -p op-ingest --bin op-fetch -- --help
```

Card data is fetched on first run by either client. Tests that need it skip
themselves when it is absent, so a bare clone is green — which also means a
green run proves less than it looks. Fetch before trusting a card-level change:

```bash
cargo run -p op-ingest --bin op-fetch -- --data-dir data --packs ST-01 ST-02 ST-06
```

**The front end is embedded at compile time.** Editing `client/` requires
`cargo build -p op-desktop`; there is no refresh-to-reload.

## Layout

| Crate | |
|---|---|
| `op-core` | The rules kernel. Knows the rules, knows nothing about individual cards. |
| `op-cards` | Card scripts, one module per product, plus the scripting DSL. |
| `op-ai` | Determinization, a hand-written evaluation, ISMCTS. |
| `op-ingest` | Fetching card data and art. |
| `op-desktop` | Tauri client; `client/` is its front end. |
| `op-cli` | Terminal client. |

## Invariants

These are load-bearing. Breaking one usually still compiles.

**Hidden information is a type boundary.** `GameState` is omniscient and must
never reach a client. Clients get `PlayerView` and `PlayerEvent`, produced by
`project`. A `CardInstanceId` is *not* an opaque token — ids are assigned in
decklist order at setup, so sending one for a hidden card leaks the card to
anyone holding the decklist. When in doubt, send nothing.

The session debug log is the deliberate exception: it records `GameEvent`, so
it holds both hands. It is local-only and must never be surfaced to a player
mid-game.

**Determinism.** A game is a pure function of `(GameConfig, seed, [Action])`.
All randomness goes through `GameState::rng`; rules paths use ordered
containers and dense ids, never `HashMap` iteration; no floating point in the
rules. Breaking this breaks replay, the debug logs, and MCTS.

**Characteristics are derived, never stored.** Power, cost and keywords are
recomputed every time from printed values plus DON!! plus modifiers plus
permanent effects. Never mutate them in place. Note `Characteristics::cost` may
be negative mid-calculation and is clamped only by `effective_cost()` (rule
1-3).

**Effect resolution suspends as data.** An `EffectFrame` carries an instruction
pointer, so `GameState` stays `Clone + Serialize` even mid-effect. Do not
introduce async or coroutines into resolution.

**A script that does nothing still passes the type checker.** Binding keys are
strings and an op that reads an unbound key gets an empty slice, so a `choose`
on `"t"` read back as `"target"` compiles, runs, and silently has no effect.
`op_core::validate::validate_script` catches that class — unbound keys, dead
bindings, reads before their `choose`, timings the engine never fires,
unsatisfiable costs — and `op-cards/tests/scripts_are_well_formed.rs` runs it
over every script. Add a check there rather than a one-off test when a new way
to write a silently-dead script appears.

**One legal-action generator.** `op_core::legal_actions` serves search, the RL
action mask, and (eventually) the server validator. Keep it faithful to the
rules — advice for the UI belongs in a separate helper, as
`activation_finds_targets` is.

## Conventions

**Cite the rule.** Non-obvious rules code carries its Comprehensive Rules
number, and conformance tests are named for the clause they pin down
(`rule_7_1_4_1_attacker_wins_ties…`). If you cannot find a clause for what you
are implementing, that is a signal.

**Check the rules text before changing rules behaviour.** Intuition about how a
TCG "obviously" works has been wrong here more than once, in both directions.
The Comprehensive Rules are published; read the clause. `python3
tools/rules/fetch_rules.py` puts them in `data/rules/` — gitignored, like the
card data, because they are Bandai's copyright. The whole document is ~10,600
words, so reading the surrounding section costs very little.

The `rules-auditor` agent (`.claude/agents/`) reviews against them: it screens a
`debug/*.jsonl` trace for illegal play, and checks a card script against its
printed text and against what `validate_script` does and does not catch. It is
read-only and cites a clause for every finding. Worth pointing at a new set's
scripts before rolling it out.

**Comments explain why, not what.** Prefer recording the reason a thing is
surprising over narrating the code.

**A test that cannot fail is not a test.** For anything security- or
correctness-critical, verify the test fails against the old behaviour before
trusting it.

## Adding a card set

1. `op-fetch --data-dir data --packs ST-03`
2. `cargo run -p op-cards --bin coverage -- ST03` to list what needs scripting
3. Add `crates/op-cards/src/sets/st03.rs`, register it in `sets/mod.rs` and
   `all_scripts()`
4. Extend the DSL in `op-cards/src/dsl.rs` (and the ops in
   `op-core/src/effect.rs`) where existing pieces do not fit
5. Repeat until the set reports `OK`
6. Bump the card-sets badge and the `Status` line at the top of `README.md` —
   `cargo run -p op-cards --bin coverage` prints the `N/59` to put in both

Keep each card's printed text in a comment above its script. Where the script
diverges from the text, say why and cite the rule.
