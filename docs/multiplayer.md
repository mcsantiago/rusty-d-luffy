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

The desktop app is the multiplayer client, not a browser. The README's status
section promises a web client, and one is cheap — the front end touches Tauri
in one place (`window.__TAURI__`, eight `invoke` calls and two `listen`
channels), so a WebSocket shim would run the same renderer in a browser, and
serving it from `op-server` would make distribution a page refresh. It is
deferred anyway, because a browser cannot do the local art fetch: the server
would have to serve ~750 MB of card images, which moves this project from a
tool that helps a user fetch Bandai's assets to a host that distributes them.
That is a licensing posture, not an implementation detail, and it is not worth
adopting to solve an update-prompt problem. Distribution is handled under
Versioning instead.

## Why not peer-to-peer

The obvious cheaper thing is to cut the server out. It does not work here, and
the reason is hidden information rather than networking.

**Lockstep is incompatible with a hidden hand.** Lockstep means both peers
simulate the same game from the same actions — which requires both peers to
hold the full state, which is the entire shuffle. It is the right answer for an
RTS, which has no real secrets, and fog-of-war maphacks are the well-known
consequence of the one place it does. This ruleset is turn-based and
deterministic, so peers would stay *synchronised* perfectly; they would simply
both know everything. (The README's status section currently offers lockstep as
sufficient for netcode. That is true of synchronisation and false of
confidentiality.)

**Host-authoritative peer-to-peer leaks to the host.** One peer runs the
engine, so that peer's process holds `GameState`: the opponent's hand, their
deck order, their life cards. That is not a leak to be patched, it is what the
type is. The host also chooses the seed, and so the shuffle.

**The cryptographic fix is out of proportion to the project.** Mental-poker
protocols genuinely solve this, but they need commitments and reveal proofs
everywhere the rules touch a hidden zone — draw, mulligan, life, `[Trigger]`
reveal, and every deck-searching effect. That machinery would be larger than
the rules engine. The cheap variant — each player commits to
`H(deck_order ‖ nonce)` at setup and reveals at the end, making cheating
*detectable afterwards* rather than impossible — is more proportionate, but
still assumes an engine in which neither side is omniscient. `GameState` is
unitary by design. That is a different engine, not a refactor.

**And peer-to-peer usually needs a server anyway.** Two peers behind
residential NAT need STUN to find each other and TURN when hole-punching fails.
TURN relays the actual traffic, so the fallback path costs more to run than
this game server does. Giving up the security model would not remove the
infrastructure.

**What does follow is that "central" is a deployment choice, not an
architectural one.** `op-server` is a binary holding matches in memory; nothing
above requires it to be always-on infrastructure. The same build supports a
homelab instance that is always up, a *host a match* button where one player's
desktop app runs the server in-process and the other connects, and LAN play
with no internet at all. The host is trusted in that second case — which for
friends is the honest security model regardless — but the authority stays in
one place, and moving it later is a deployment change rather than a rewrite.

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
  anything else. See Versioning below.

Ranked play, ratings and persistent accounts are out of scope until there is a
reason for them.

## Versioning and updates

The desktop client is distributed as a binary and `client/` is embedded at
compile time, so a stale install is normal. Under multiplayer that stops being
harmless: a client the server will not talk to cannot play at all. The plan is
a version check against the GitHub releases page, prompting the user to
download a newer build, plus a server that refuses clients it disagrees with.

The thing to keep straight is that **three versions move independently**, and
gating on the wrong one either locks players out for no reason or lets a
broken client connect.

| | Changes when | Enforcement |
|---|---|---|
| `PROTOCOL_VERSION` (`op-proto`) | the wire format changes | **hard** — server refuses |
| `card_data_ref` | the card-data pin moves | warn at connect |
| Release tag | every build | advisory prompt only |

**Gate on the protocol, not the release tag.** They are tempting to conflate
because the release is what a user actually downloads, but a UI fix or a card
script bumps the release tag without touching the wire format. Gating on the
tag would lock out everyone who had not updated, for a change that could not
possibly have broken them. `PROTOCOL_VERSION` is a separate constant, bumped
only when the messages change.

**Card scripts are not a coupling.** In multiplayer only the server runs the
engine, so its `op-cards` is the only copy whose behaviour matters. A client
with older scripts plays fine; it needs card *data* for names, text and art,
which is what `card_data_ref` covers. Mismatched data means the client renders
stale text next to power the server derived — cosmetic, not a desync, but
worth saying at connect rather than mid-battle.

**The handshake is the source of truth; GitHub is a convenience.** A rejection
should carry the protocol version the server wants and where to get a build, so
the message is right even when the releases API is unreachable. The GitHub
check is a pre-flight that turns "connection refused" into "there is a newer
version" before the user picks a deck.

Details that decide whether the check is pleasant or annoying:

- **Check on entering multiplayer, not at launch.** Solo play stays entirely
  offline, which is a property the app currently advertises and should keep.
- **Fail silently.** Unauthenticated GitHub allows 60 requests an hour per IP,
  and users behind one NAT share it. A check that errors, rate-limits or times
  out must never block anything — worst case the handshake catches it.
- **Compare parsed versions, not strings.** `0.10.0` sorts below `0.9.0`
  lexicographically.
- **Use `/releases/latest`**, which already excludes drafts and prereleases,
  and confirm the release actually has an asset for the user's platform before
  offering it — CI may still be building one.

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
- **Spectators.** `PlayerView::project` takes a seat; a neutral view that hides
  both hands is not currently expressible. Related to #47, which has the same
  question in the form of "replay as one seat rather than omnisciently."
