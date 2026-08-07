// Front end for the One Piece Card Game simulator.
//
// This is a renderer. It never sees GameState — only a PlayerView, a log of
// already-projected events, and the legal actions available to the human. The
// same shape will come off a socket in multiplayer, so this file should not
// need to change when that lands.

// Surfaced in the UI rather than only the console: a throw at module scope
// stops every listener from being attached, which looks like buttons doing
// nothing at all.
function fatal(message) {
  const box = document.getElementById("setup-error");
  if (box) box.textContent = message;
  console.error(message);
}

window.addEventListener("error", (e) => fatal(`${e.message}`));
window.addEventListener("unhandledrejection", (e) => fatal(`${e.reason}`));

if (!window.__TAURI__ || !window.__TAURI__.core) {
  fatal("Tauri API unavailable — is `withGlobalTauri` set in tauri.conf.json?");
  throw new Error("missing window.__TAURI__");
}

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

/** Printed card details, keyed by card number, sent once per game. */
let catalogue = new Map();
/** Card art data URIs, resolved lazily and cached for the session. */
const artCache = new Map();
/** Instance ids the hovered action refers to, for highlighting. */
let highlighted = new Set();
/** The cards in the battle currently being resolved, if any. */
let battleAttacker = null;
let battleDefender = null;

const $ = (id) => document.getElementById(id);

// ---- card art ---------------------------------------------------------------

async function art(number) {
  if (!number) return null;
  if (artCache.has(number)) return artCache.get(number);
  const promise = invoke("card_art", { number }).catch(() => null);
  artCache.set(number, promise);
  return promise;
}

// ---- rendering --------------------------------------------------------------

/** Builds a card element. `card` is a VisibleCard from the engine. */
function cardEl(card, { small = false } = {}) {
  const el = document.createElement("div");
  el.className = "card" + (small ? " small" : "");
  el.dataset.id = String(card.id);
  if (card.rested) el.classList.add("rested");
  if (highlighted.has(card.id)) el.classList.add("highlight");
  // A battle is easy to lose track of once the log has scrolled, and the
  // Counter step asks you to judge a matchup you cannot otherwise see.
  if (card.id === battleAttacker) el.classList.add("attacking");
  if (card.id === battleDefender) el.classList.add("defending");

  if (!card.number) {
    // A card the viewer may not identify: the engine sent no number at all.
    el.classList.add("facedown");
    el.innerHTML = `<div class="back"></div>`;
    return el;
  }

  const info = catalogue.get(card.number);
  el.innerHTML = `
    <div class="art"></div>
    <div class="fallback">
      <div class="cname">${info ? info.name : card.number}</div>
      <div class="cnum">${card.number}</div>
    </div>
    <div class="badges"></div>
  `;

  const badges = el.querySelector(".badges");
  if (card.power !== null && card.power !== undefined) {
    badges.insertAdjacentHTML("beforeend", `<span class="pw">${card.power}</span>`);
  }

  art(card.number).then((uri) => {
    if (!uri) return;
    const a = el.querySelector(".art");
    if (a) a.style.backgroundImage = `url("${uri}")`;
    el.classList.add("has-art");
  });

  el.addEventListener("mouseenter", () => showPreview(card.number));
  el.addEventListener("mouseleave", hidePreview);
  return el;
}

async function showPreview(number) {
  const info = catalogue.get(number);
  const box = $("preview");
  const uri = await art(number);
  box.innerHTML = `
    ${uri ? `<img src="${uri}" alt="${number}" />` : ""}
    <div class="ptext">
      <div class="pname">${info ? info.name : number}</div>
      <div class="pmeta">${info ? `${info.category} · cost ${info.cost}${
        info.power != null ? ` · ${info.power}` : ""
      }${info.counter != null ? ` · counter ${info.counter}` : ""}` : number}</div>
      ${info && info.effect ? `<div class="peffect">${info.effect.replaceAll("<br>", "<br/>")}</div>` : ""}
      ${info && info.trigger ? `<div class="ptrigger">${info.trigger}</div>` : ""}
    </div>
  `;
  box.hidden = false;
}

function hidePreview() {
  $("preview").hidden = true;
}

/** A card plus any DON!! given to it.
 *
 * DON!! given to a card are placed underneath it and stay visible (6-5-5-1),
 * and they rest and rotate along with it — so the rotation lives on this
 * wrapper rather than on the card itself.
 */
function cardSlot(card, opts = {}) {
  const slot = document.createElement("div");
  slot.className = "slot" + (card.rested ? " rested" : "");

  if (card.attached_don > 0) {
    const stack = document.createElement("div");
    stack.className = "attached";
    for (let i = 0; i < card.attached_don; i++) {
      const d = document.createElement("div");
      d.className = "adon";
      // Fan sideways so each given DON!! is individually countable.
      d.style.left = `${i * 13}px`;
      d.style.zIndex = String(i);
      stack.appendChild(d);
    }
    slot.appendChild(stack);
  }

  slot.appendChild(cardEl(card, opts));
  return slot;
}

/** A single DON!! card. Fungible and art-less, so drawn rather than imaged. */
function donEl(card) {
  const el = document.createElement("div");
  el.className = "don-card" + (card.rested ? " rested" : "");
  el.dataset.id = String(card.id);
  if (highlighted.has(card.id)) el.classList.add("highlight");
  el.innerHTML = `<span>DON</span><span class="bangs">!!</span>`;
  return el;
}

function renderDon(side, prefix, deckCount) {
  const row = $(`${prefix}-don-row`);
  row.innerHTML = "";

  if (side.don.length === 0) {
    row.innerHTML = `<div class="empty">no DON!!</div>`;
  }
  // Active first so the usable pool reads at a glance.
  const ordered = [...side.don].sort((a, b) => Number(a.rested) - Number(b.rested));
  for (const d of ordered) row.appendChild(donEl(d));

  const remaining = document.createElement("div");
  remaining.className = "don-deck";
  remaining.title = "DON!! deck";
  remaining.textContent = deckCount;
  row.appendChild(remaining);
}

function renderSide(view, side, prefix) {
  const leader = $(`${prefix}-leader`);
  leader.innerHTML = "";
  if (side.leader) leader.appendChild(cardSlot(side.leader));

  const chars = $(`${prefix}-characters`);
  chars.innerHTML = "";
  if (side.characters.length === 0) {
    chars.innerHTML = `<div class="empty">no characters</div>`;
  }
  for (const c of side.characters) chars.appendChild(cardSlot(c));

  const stage = $(`${prefix}-stage`);
  stage.innerHTML = "";
  if (side.stage) stage.appendChild(cardSlot(side.stage, { small: true }));
}

function lifePips(n) {
  return `<span class="pips">${"●".repeat(n)}${"○".repeat(Math.max(0, 5 - n))}</span> ${n}`;
}

/** Every card on the board, by instance id, for looking up live power. */
function boardIndex(view) {
  const index = new Map();
  for (const side of [view.you, view.opponent]) {
    for (const c of [side.leader, side.stage, ...side.characters]) {
      if (c) index.set(c.id, c);
    }
  }
  return index;
}

function renderBattle(view) {
  const bar = $("battle-bar");
  if (!view.battle) {
    bar.hidden = true;
    return;
  }
  const index = boardIndex(view);
  const attacker = index.get(view.battle.attacker);
  const defender = index.get(view.battle.target);
  const label = (c) => {
    if (!c) return "?";
    const info = c.number ? catalogue.get(c.number) : null;
    const nm = info ? info.name : (c.number ?? "?");
    return c.power != null ? `${nm} <b>${c.power}</b>` : nm;
  };
  bar.hidden = false;
  bar.innerHTML = `
    <span class="atk">${label(attacker)}</span>
    <span class="arrow">&#8594;</span>
    <span class="def">${label(defender)}</span>
    <span class="step">${view.battle.step}</span>
  `;
}

function render(snap) {
  const view = snap.view;
  battleAttacker = view.battle ? view.battle.attacker : null;
  battleDefender = view.battle ? view.battle.target : null;

  $("turn-label").textContent = snap.turn_label;
  $("phase-label").textContent = `${view.phase} phase · turn ${view.turn}`;

  $("opp-life").innerHTML = `life ${lifePips(view.opponent.life_count)}`;
  $("opp-hand").textContent = `hand ${view.opponent.hand_count}`;
  $("opp-deck").textContent = `deck ${view.opponent.deck_count}`;
  $("opp-don").textContent = `${view.opponent.don_active} active DON!!`;

  $("you-life").innerHTML = `life ${lifePips(view.you.life_count)}`;
  $("you-deck").textContent = `deck ${view.you.deck_count}`;
  $("you-don").textContent = `${view.you.don_active} active DON!!`;

  renderSide(view, view.opponent, "opp");
  renderSide(view, view.you, "you");
  renderDon(view.opponent, "opp", view.opponent.don_deck);
  renderDon(view.you, "you", view.you.don_deck);

  const hand = $("hand");
  hand.innerHTML = "";
  if (view.you.hand.length === 0) {
    hand.innerHTML = `<div class="empty">hand empty</div>`;
  }
  for (const c of view.you.hand) hand.appendChild(cardSlot(c));

  renderBattle(view);

  $("question").textContent =
    snap.question ?? (snap.thinking ? "Opponent is thinking…" : "Waiting…");

  const options = $("options");
  options.innerHTML = "";
  if (snap.thinking) {
    options.innerHTML = `<div class="thinking">Opponent is thinking…</div>`;
  }
  for (const opt of snap.options) {
    const b = document.createElement("button");
    b.className = `opt ${opt.kind}`;
    b.textContent = opt.label;
    b.addEventListener("click", () => choose(opt.index));
    b.addEventListener("mouseenter", () => {
      highlighted = new Set(opt.cards);
      applyHighlight();
    });
    b.addEventListener("mouseleave", () => {
      highlighted = new Set();
      applyHighlight();
    });
    options.appendChild(b);
  }

  const log = $("log");
  log.innerHTML = snap.log
    .slice(-80)
    .map((l) => `<div class="entry${l.startsWith("—") ? " turn" : ""}">${l}</div>`)
    .join("");
  log.scrollTop = log.scrollHeight;

  if (snap.over) {
    $("result-text").textContent = snap.over;
    $("result").hidden = false;
  }
}

/** Toggles the highlight class in place.
 *
 * Deliberately not a re-render: re-rendering would recreate the option button
 * currently under the cursor, re-firing mouseenter and looping forever.
 */
function applyHighlight() {
  for (const el of document.querySelectorAll(".card, .don-card")) {
    el.classList.toggle("highlight", highlighted.has(Number(el.dataset.id)));
  }
}

let lastSnapshot = null;

// ---- actions ----------------------------------------------------------------

async function choose(index) {
  $("options").innerHTML = `<div class="thinking">Opponent is thinking…</div>`;
  try {
    // Returns as soon as your own move is applied. If the AI owes a reply it
    // is computed on a worker and arrives later via game://update, so the
    // board shows your move immediately rather than freezing until the search
    // finishes.
    lastSnapshot = await invoke("choose", { index });
    render(lastSnapshot);
  } catch (err) {
    $("question").textContent = String(err);
  }
}

listen("game://update", (event) => {
  lastSnapshot = event.payload;
  render(lastSnapshot);
});

async function start() {
  $("setup-error").textContent = "";
  try {
    const result = await invoke("new_game", {
      seed: null,
      yourDeck: $("your-deck").value,
      aiDeck: $("ai-deck").value,
      difficulty: $("difficulty").value,
      youFirst: $("you-first").checked,
    });
    catalogue = new Map(result.catalogue.map((c) => [c.number, c]));
    artCache.clear();
    lastSnapshot = result.snapshot;
    $("setup").hidden = true;
    $("result").hidden = true;
    $("game").hidden = false;
    render(lastSnapshot);
  } catch (err) {
    $("setup-error").textContent = String(err);
  }
}

// ---- startup ----------------------------------------------------------------
//
// The window opens before card data exists. On a fresh checkout `data/` is
// empty, so the backend fetches it on a worker thread and streams progress
// here. The setup panel stays on screen and interactive throughout; only
// starting a game is gated, since that is the one thing that genuinely needs
// the data.

function setIngestVisible(visible) {
  $("ingest").hidden = !visible;
  // Gated for the whole download, not just until card data parses. Art
  // arrives across all 59 sets in no particular order, so starting early can
  // show text placeholders instead of the cards in your own deck.
  $("start").disabled = visible;
  $("start").textContent = visible ? "Downloading…" : "Start game";
}

function appendIngestLine(line) {
  const log = $("ingest-log");
  log.textContent += (log.textContent ? "\n" : "") + line;
  log.scrollTop = log.scrollHeight;
}

async function bootstrap() {
  $("ingest-retry").hidden = true;
  let status;
  try {
    status = await invoke("bootstrap");
  } catch (err) {
    setIngestVisible(true);
    $("ingest-title").textContent = "Could not start";
    appendIngestLine(String(err));
    $("ingest-retry").hidden = false;
    return;
  }

  if (status.ready) {
    setIngestVisible(false);
    return;
  }
  setIngestVisible(true);
  $("ingest-title").textContent = status.message;
  if (!status.fetching) {
    appendIngestLine(status.message);
    $("ingest-retry").hidden = false;
  }
}

const PHASE_LABELS = { packs: "card data", images: "card art" };

listen("ingest://progress", (event) => {
  const { line, done, ok, phase, current, total } = event.payload;

  // Counter updates carry no text; they drive the bar instead of the log, so a
  // few thousand of them do not drown the useful output.
  if (phase && total > 0) {
    const pct = Math.round((current / total) * 100);
    $("ingest-bar").style.width = `${pct}%`;
    $("ingest-count").textContent =
      `${PHASE_LABELS[phase] ?? phase}: ${current} / ${total} (${pct}%)`;
    return;
  }

  if (line) appendIngestLine(line);
  if (!done) return;

  $("ingest").classList.toggle("failed", !ok);
  $("ingest-title").textContent = ok ? "Card data ready" : "Fetch failed";
  if (ok) {
    $("ingest-bar").style.width = "100%";
    $("ingest-count").textContent = "complete";
  }
  if (ok) {
    // The backend only reports done-ok once the data is parsed, so this is a
    // genuine ready signal rather than merely "downloaded".
    setTimeout(bootstrap, 400);
  } else {
    $("ingest-retry").hidden = false;
    $("start").disabled = true;
  }
});

$("ingest-retry").addEventListener("click", () => {
  $("ingest-log").textContent = "";
  $("ingest-bar").style.width = "0%";
  $("ingest-count").textContent = "";
  $("ingest").classList.remove("failed");
  bootstrap();
});

bootstrap();

$("start").addEventListener("click", start);
$("new-game").addEventListener("click", () => {
  $("game").hidden = true;
  $("setup").hidden = false;
});
$("result-new").addEventListener("click", () => {
  $("result").hidden = true;
  $("game").hidden = true;
  $("setup").hidden = false;
});
