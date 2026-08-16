# Deck building

Notes for work on `op-deck` and the clients that use it. Covers the decisions
that are not obvious from one file, and the reasons the pipeline is split the
way it is.

Tracking issue: [#93](https://github.com/mcsantiago/rusty-d-luffy/issues/93).

## Four questions, in order

A pasted decklist is not one thing to validate. It is four, and a user hitting
the third needs a different message from one hitting the fourth:

| Stage | Module | Question | Needs |
|---|---|---|---|
| Parse | `text` | Is this a decklist? | nothing |
| Resolve | `resolve` | Do we have these cards? | `CardDb` |
| Legality | `legality` | Is it a legal deck? (5-1-2) | `CardDb` |
| Compatibility | `compat` | Can this build play it? | `CardSupport` |

Each stage passes its output to the next and none of them fail closed: a list
with one bad line still yields the other 49 entries, an unresolved card still
counts toward the deck size, and an unsupported card is still a legal deck. The
UI is expected to show all four results at once.

**The distinctions that matter, and why they cost something to keep:**

*Unparseable vs. unknown.* `text` deliberately does not know which cards exist,
so `STO1-002` (letter O for zero) parses fine and resolves to nothing. That is
the point: the parser cannot tell a typo from a pack you have not fetched, and
those need opposite advice. Whether a token *looks* like a card number is
structural — it contains a `-` and is otherwise alphanumeric.

*Illegal vs. unsupported.* An off-colour deck is illegal at a real table and
must be refused. A deck full of unscripted cards is perfectly legal and simply
will not play correctly here. Collapsing the two would either block legal decks
or ship silently-wrong games.

*Unsupported vs. unknown.* A card with text and no script plays as a vanilla
body — its text silently does nothing, which is how you lose a game without
knowing why. A card that is not in the database at all may just need
`op-fetch`. Different fixes, different buckets.

## Engine support lives in `op-cards`

`op-deck` defines the vocabulary (`Support`, `CardSupport`) and `op-cards`
implements it, so `op-cards` depends on `op-deck` rather than the other way
round. That inversion is deliberate: deck handling must not depend on the
script corpus, or the deck crate would rebuild every time a card was added.

`Cards::coverage` is the single classifier, and it has two consumers — the
`coverage` binary reports it per set, `op_deck::compat` per deck. They were
always answering the same question; two implementations would have drifted the
first time a new way to be unplayable appeared. A card is `Partial` when it has
a script that `validate_script` finds fault with, which is the only honest
signal available for "partially implemented".

## Entry order is load-bearing

Setup assigns `CardInstanceId`s by walking the decklist, so the *order* of a
deck decides which game a seed produces — and therefore whether a session log
still replays. Every stage here preserves it:

- `text::parse` merges a repeated card number into its **first** mention.
- JSON arrays are ordered, so a saved deck round-trips through disk unchanged.
- `expand` emits each entry's copies as a contiguous run.

`collapse` is the inverse of `expand` only for lists whose copies are already
contiguous, which is how everything in this project builds one. It is for
loading a deck into an editor, not for round-tripping one through a game.

## Deck ids are filenames

`DeckId` is a slug derived from the deck's name and constrained to
`[a-z0-9-]`. It is the filename, so the constraint is load-bearing rather than
cosmetic — a deck called `../../etc/passwd` must not decide where the file
lands. Ids do not move when a deck is renamed: the id is the deck's identity
and anything already pointing at it would break.

Saves are written to a temporary file and renamed, and a deck file that fails
to parse is skipped by `list` rather than failing it. One corrupt deck should
cost its owner that deck, not the collection.

## What the engine does not check

`Game::new`'s `validate_deck` enforces 5-1-2 (50 cards) and 5-1-2-3 (4 copies)
and nothing else. It does **not** check 5-1-2-2 (a card must share a colour
with the Leader) or 5-1-2-1 (a deck is Characters, Events and Stages). Those
live only in `op_deck::legality`, so any path that reaches `Game::new` without
passing through it — a decklist file, a test fixture — can build an illegal
deck that the engine plays without complaint.

Whether to move those checks into the kernel is open. The argument against is
that kernel tests deliberately build tiny illegal decks and would all need
`allow_illegal_decks`; the argument for is that a rule enforced in one place
only is a rule waiting to be bypassed.

## Still to build

- **Deck builder UI** (issue phase 3) — card browser with search and filters,
  editable deck list, live legality and compatibility. Nothing in `op-deck`
  assumes a particular front end.
- **Match integration** (issue) — the setup screen should offer saved decks
  rather than `op_cards::decks::ALL`. The built-in lists stay as seed content:
  `collapse` turns one into `DeckEntry`s so a starter deck can be duplicated
  and edited.
- **Launch policy for unsupported decks** — the issue asks for either a block
  with an explanation or an explicit "allow unsupported cards" testing mode.
  `DeckCompatibility::summary().is_complete()` is the predicate; the policy is
  not yet wired anywhere.
