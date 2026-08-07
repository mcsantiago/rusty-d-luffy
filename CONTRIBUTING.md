# Contributing

Contributions are welcome. See [AGENTS.md](AGENTS.md) for how the codebase is
organised and which invariants are easy to break.

## AI assistance

**AI assistance is welcome and does not need to be disclosed line by line.**
Much of this project was written with it, and the commit history says so.

What matters is not how a change was produced but that you stand behind it:

- **You understand what you are submitting.** If you cannot explain why a
  change is correct, it is not ready — and that applies equally to code you
  typed yourself.
- **You have run it.** `cargo fmt --all`, `cargo clippy --workspace
  --all-targets`, and `cargo test --workspace --release` all pass.
- **Rules changes cite the rule.** A behaviour change in `op-core` needs a
  Comprehensive Rules clause and a test named for it. This is where generated
  code most often goes confidently wrong: plausible-sounding TCG behaviour that
  the actual rules text contradicts.

Reviews judge the change, not its provenance.

## Before opening a pull request

- `cargo fmt --all` — CI rejects unformatted code
- `cargo clippy --workspace --all-targets` — CI runs it with `-D warnings`
- `cargo test --workspace --release`
- Fetch card data first if you touched anything card-related; those tests skip
  themselves without it, so a green run can be hollow

For a rules or card change, add a test that **fails without your change**. For
anything touching the hidden-information boundary, verify that explicitly — a
leak test that cannot fail is worse than none, because it looks like coverage.

## Commit messages

Explain the reasoning, not the diff. What was wrong, why the fix is the right
one, and what you considered and rejected. Long is fine; the history is the
main record of why this codebase is shaped the way it is.

## Licensing

The project is [AGPL-3.0](LICENSE). By contributing you agree your
contributions are licensed under it.

Note the AGPL covers this source only. Do not add card text, card images, or
any other third-party material to the repository — it is fetched at runtime and
deliberately never vendored. See [NOTICE](NOTICE).

## Scope

This is an unofficial fan project. Please do not open issues asking for
features that would involve redistributing Bandai's assets, or that would put
the project on a commercial footing.
