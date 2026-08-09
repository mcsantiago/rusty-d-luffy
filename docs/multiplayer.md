# Multiplayer architecture

Status: **design, nothing built.** Tracking issue: #49.

Two players, each on their own machine, playing an authoritative match against
a rules engine neither of them runs. A WebSocket server holds the game; clients
hold a rendering of it and nothing else.

This document covers what the engine already provides, what has to be built,
and the handful of decisions that are load-bearing enough to get wrong quietly.

## Why the kernel is already shaped for this

Very little of this is new architecture. The pieces were built with a server in
mind and say so:

- **A game is a pure function of `(GameConfig, seed, [Action])`.** The server
  needs no state beyond that tuple to be authoritative, and no database to
  recover a match.
- **The projection boundary already exists.** `PlayerView::project`
  (`view.rs:77`) and `GameEvent::project` (`event.rs:318`) redact to one seat;
  `StepOutcome::for_player` (`game.rs:79`) does both at once and is documented
  as "safe to send to them." `view.rs` opens by asserting the server's send
  path takes a `PlayerView` — that sentence is currently aspirational, and this
  is the work that makes it true.
- **One legal-action generator.** AGENTS.md already names `legal_actions` as
  serving "(eventually) the server validator."
- **The authorization check is prototyped.** `Session::apply_human`
  (`session.rs:302`) rejects a decision that isn't yours, then indexes into
  `legal_actions` rather than trusting a caller-supplied action. That is
  precisely the server's inner loop, with one seat instead of two.
- **Persistence is already a solved problem.** The session log is a complete
  reproducer. A server that writes one can rebuild any match after a restart —
  the same machinery #47 needs for its replay cursor.

## Shape

```
                    ┌─────────────────────────────────┐
   op-desktop ──ws──┤  op-server                      │
   op-desktop ──ws──┤    match registry (in memory)   │
                    │    matchmaking queue            │
                    │    op-core + op-cards           │
                    │    session log per match ───────┼──▶ PVC
                    └─────────────────────────────────┘
```

**`op-proto`** — new crate. The wire types, versioned, depended on by both
sides so they cannot drift. Depends on `op-core` for `Action`, `PlayerView`,
`PlayerEvent` and `Pending`, all of which already derive `Serialize`.
`StepOutcome` and `PlayerOutcome` do not; either add the derives or mirror them
in `op-proto`.

**`op-server`** — new crate. axum + tokio, one WebSocket endpoint, an in-memory
registry of live matches, a matchmaking queue. Owns every `Game`. Writes a
session log per match.

**`op-desktop`** — gains a second backend behind the interface `Session`
already implies (snapshot, offer, apply, is-it-my-turn). Solo-vs-AI keeps the
local engine: it stays instant and offline, and it keeps ISMCTS off the server
(see Deployment). If that dual path proves annoying to keep in sync, the
fallback is a thin client with an in-process server for solo — but two
backends behind one interface is the cheaper first move.

## The client cannot compute its own options

`legal_actions(game: &Game)` takes the omniscient game. A client does not have
one and must never have one, so **the server has to send the offered options**;
the client cannot derive them from a `PlayerView`. This is not a limitation to
work around, it is the design:

Server → client, on every decision point:

```
View     { view: PlayerView }
Events   { events: Vec<PlayerEvent> }
Offer    { offer_id: u64, options: Vec<Choice> }
```

Client → server:

```
Choose   { offer_id: u64, index: usize }
```

`Choice` is roughly the desktop's existing struct (`session.rs:42`) — label,
involved cards, kind — which is already the right payload for a UI and is
built server-side, so labelling logic stays in one place.

Replying with an **index into an offer** rather than a raw `Action` deletes an
entire class of attack: a malformed action, an action naming a card the sender
cannot see, and an action that is merely illegal all become impossible to
express. The `offer_id` must be checked and must increase — otherwise a
duplicated or delayed reply applies to whatever decision happens to be current.

If a raw-`Action` path is ever needed, validate by **membership in
`legal_actions`**, not by `Game::step` returning `Ok`. `step`'s rejection is a
rules check, not an authorization one.

## The rules that are load-bearing

**The seed is a secret.** Seed plus decklist is the entire shuffle — every draw
both players will make, in order. Today `Session` derives its human-readable
id from it (`session_id: format!("{:08x}", seed as u32)`, `session.rs:273`) and
the UI prints it (`client/app.js:1145`). Doing the same thing for a match code
would hand both players the whole game. In multiplayer the seed never leaves
the server, and the match code is generated independently of it.

**`Action` carries no `PlayerId`.** The actor is implicit — whoever `Pending`
says owes the decision. `Game::step` will apply whatever it is handed as that
player's decision, so nothing in the engine can stop seat B from acting as seat
A. The server must check `pending.player() == connection.seat` on every message
before stepping. This is the single check that, if missed, still compiles,
still passes every existing test, and loses every game to whoever notices.

**Instance-id leakage stops being theoretical.** Ids are assigned in decklist
order at setup, so an id for a hidden card identifies it to anyone who knows
the decklist — and the built-in decklists are in this repo. Solo play has no
adversary; multiplayer hands one an arbitrary client. The projection already
handles this properly (`CardRef::Hidden`, `event.rs:340`; the opponent's `hand`
is always empty in `PlayerSide`), so the requirement is to keep it that way:
a projection test that fails when any hidden card's id appears in the projected
output, run over whole games rather than hand-written positions.

**`GameState` must never be nameable on the wire.** `op-proto` should not
depend on it, so that sending one is a compile error rather than a review
comment.

**Timeouts and concedes must be actions.** The engine has no clock and must not
grow one — a turn timer lives in the server. But if a timeout or a concede
resolves the game outside the action stream, the log stops being a reproducer
and #47's replay breaks. Both need to enter the engine as `Action`s. There is
no `Concede` variant today; one has to be added, with the rules citation for
what conceding does to the game result.

## Matchmaking

Small and boring on purpose:

- **Queue** — join, get paired with the next waiting player, deck chosen before
  entering. Server validates both decks with `allow_illegal_decks: false`.
- **Private code** — create a match, get a short code, share it out of band.
  Generated independently of the seed.
- **Handshake** — protocol version and `card_data_ref` exchanged before
  anything else. A client on different card data derives different power and
  will disagree with the server about the board; since the server is
  authoritative that is cosmetic rather than a desync, but it should be
  reported at connect rather than discovered mid-battle.

Ranked play, ratings and persistent accounts are out of scope until there is a
reason for them.

## Deployment

**A match is stateful and pinned to a pod.** The WebSocket makes that automatic
— the connection *is* the affinity — but it means the usual stateless
assumptions do not hold.

- **Rolling updates end matches.** Mitigate in two steps: on SIGTERM stop
  accepting new matches and let existing ones finish (a match is ~10–20
  minutes, so `terminationGracePeriodSeconds` has to be generous); and, later,
  recover from the session log on restart, which is the same rebuild-from-
  actions path #47 needs.
- **One replica is correct to start.** Scaling out needs match-id-based routing
  to a specific pod; do not reach for it before there is load to justify it.
- **Keep ISMCTS off the server.** A search is expensive enough that the test
  suite budgets ~60s per match fanned across cores. Solo-vs-AI stays on the
  client. If server-hosted AI is ever wanted, it belongs in its own deployment
  with its own CPU limits and a queue, not sharing a pod with live matches.
- **Bake card data into the image** at the pinned `SOURCE_REF` rather than
  fetching on pod start — no network dependency in the startup path, and the
  image is then reproducible. Readiness = card data loaded and scripts
  registered.
- **Session logs are omniscient.** They hold both hands. They must not go to a
  shared log aggregator, and they must not be readable by a running match's
  participants. Local volume, treated as sensitive.

## Phasing

1. `op-proto`: wire types, version constant, missing serde derives.
2. `op-server`: one endpoint, in-memory registry, first-come pairing, no lobby.
   Two clients can finish a game.
3. `op-desktop`: remote backend behind the existing session-shaped interface.
4. Matchmaking: queue, private codes, deck selection and validation.
5. Robustness: reconnect (rebuild from log), turn clocks, `Action::Concede`.
6. Kubernetes: image with baked card data, Deployment, Service, ingress,
   graceful drain.

## Open questions

- **Identity.** Display names and match codes are enough for friends-only play
  over a private network. Accounts imply a user store, credential handling and
  a much larger surface.
- **Exposure.** Private (LAN or Tailscale/WireGuard) versus public ingress
  changes the TLS, rate-limiting and abuse story entirely.
- **Which client goes first.** This document assumes `op-desktop`, because it
  already renders a board and already has the session-shaped interface to hide
  a backend behind. The README's status section instead promises "the
  authoritative multiplayer server and web client." Nothing here rules a web
  client out — the server is transport-first and the front end under `client/`
  is already HTML — but the two documents currently name different targets, and
  one of them should give.
- **Spectators.** `PlayerView::project` takes a seat; a neutral view that hides
  both hands is not currently expressible. Related to #47, which has the same
  question in the form of "replay as one seat rather than omnisciently."
