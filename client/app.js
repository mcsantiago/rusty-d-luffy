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
/** The card whose menu is open, or null. */
let selected = null;
/** Whether that menu was pinned by a click, which hover then cannot displace. */
let pinned = false;
/** Actions by subject id, plus the card-less remainder, from the last render. */
let menus = new Map();
let cardless = [];
/** Whether a card with no menu is genuinely unable to act right now. */
let cardsCanAct = false;
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

/** Builds a card element. `card` is a VisibleCard from the engine.
 *
 * `yours` marks a card you control, which is the only kind that can look
 * inert: the opponent's board offers you nothing by definition, and dimming
 * all of it would say something about their position that it does not mean.
 *
 * `plain` drops the menu. Used inside overlays, which are painted above it,
 * and for the trash pile, whose own click opens the pile.
 */
function cardEl(
  card,
  { small = false, large = false, yours = false, plain = false } = {},
) {
  const el = document.createElement("div");
  el.className = "card" + (small ? " small" : "") + (large ? " large" : "");
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

  if (plain) {
    // No menu here, so the old hover panel is still how these are read.
    el.addEventListener("mouseenter", () => showPreview(card.number));
    el.addEventListener("mouseleave", hidePreview);
    return el;
  }

  // Keyed to the decision, not to whether any card happens to have a menu:
  // with no DON!! left, nothing is playable and every card should dim, which
  // is exactly when a "some card has a menu" test switches the dimming off.
  if (cardsCanAct && yours && !menus.has(card.id)) {
    el.classList.add("inert");
  }
  if (card.id === selected) el.classList.add("selected");

  el.addEventListener("mouseenter", () => hoverMenu(card, el, yours));
  el.addEventListener("mouseleave", unhoverMenu);
  el.addEventListener("click", (e) => {
    e.stopPropagation();
    pinMenu(card, el, yours);
  });
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

// ---- card menu --------------------------------------------------------------
//
// Hovering a card shows its full-size view and the actions that start from it;
// clicking pins that, so it can be read without holding the mouse still. The
// flat list is still in the sidebar, so nothing here is the only route to a
// legal action.
//
// Both edges are delayed. Opening waits so that sweeping the cursor across a
// row does not fire a menu per card; closing waits because the cursor has to
// cross a gap to reach the menu, and a menu that vanishes on the way is one
// you cannot click.

const OPEN_DELAY = 90;
const CLOSE_DELAY = 180;
let openTimer = null;
let closeTimer = null;

function hoverMenu(card, el, yours) {
  clearTimeout(closeTimer);
  if (pinned || selected === card.id) return;
  clearTimeout(openTimer);
  openTimer = setTimeout(() => showMenu(card, el, yours), OPEN_DELAY);
}

function unhoverMenu() {
  clearTimeout(openTimer);
  if (pinned) return;
  closeTimer = setTimeout(closeMenu, CLOSE_DELAY);
}

/** A click pins the menu where hover would have let it go. Clicking the
 *  pinned card again releases it. */
function pinMenu(card, el, yours) {
  clearTimeout(openTimer);
  clearTimeout(closeTimer);
  if (pinned && selected === card.id) {
    closeMenu();
    return;
  }
  pinned = true;
  showMenu(card, el, yours);
}

function showMenu(card, el, yours) {
  selected = card.id;
  for (const c of document.querySelectorAll(".card.selected")) {
    c.classList.remove("selected");
  }
  el.classList.add("selected");
  openMenu(card, el, yours);
}

function closeMenu() {
  clearTimeout(openTimer);
  clearTimeout(closeTimer);
  selected = null;
  pinned = false;
  $("card-menu").hidden = true;
  $("card-menu").classList.remove("pinned");
  for (const c of document.querySelectorAll(".card.selected")) {
    c.classList.remove("selected");
  }
}

async function openMenu(card, el, yours) {
  const menu = $("card-menu");
  const info = card.number ? catalogue.get(card.number) : null;
  const options = menus.get(card.id) ?? [];
  const uri = await art(card.number);

  // The click may have been superseded while the art resolved.
  if (selected !== card.id) return;

  menu.innerHTML = `
    <div class="menu-card">
      ${uri ? `<img src="${uri}" alt="${card.number}" />` : ""}
      <div class="menu-name">${info ? info.name : (card.number ?? "Face-down")}</div>
      <div class="menu-meta">${
        info
          ? `${info.category} · cost ${info.cost}${
              card.power != null ? ` · ${card.power} now` : ""
            }`
          : ""
      }</div>
      ${info && info.effect ? `<div class="peffect">${info.effect.replaceAll("<br>", "<br/>")}</div>` : ""}
      ${info && info.trigger ? `<div class="ptrigger">${info.trigger}</div>` : ""}
    </div>
    <div class="menu-actions"></div>
  `;

  const list = menu.querySelector(".menu-actions");
  // "Nothing from here" is worth saying about your own card and not about the
  // opponent's, where it is never news. Theirs is a preview and nothing more.
  if (options.length === 0 && yours) {
    list.innerHTML = `<div class="menu-none">No actions from this card</div>`;
  }
  fillOptions(list, options);
  list.hidden = options.length === 0 && !yours;

  menu.classList.toggle("pinned", pinned);
  menu.hidden = false;
  place(menu, el);
}

/** Anchors the menu beside `el`, kept inside the window.
 *
 * Measured after unhiding, because a hidden element has no height and the
 * hand row — where the menu must open upwards — is exactly where getting that
 * wrong pushes it off the bottom of the screen.
 */
function place(menu, el) {
  const card = el.getBoundingClientRect();
  const box = menu.getBoundingClientRect();
  const margin = 8;

  let left = card.left + card.width / 2 - box.width / 2;
  left = Math.max(margin, Math.min(left, window.innerWidth - box.width - margin));

  // Prefer above the card, which is where the hand is looked at from; fall
  // back to below when the card is near the top of the board.
  let top = card.top - box.height - margin;
  if (top < margin) top = Math.min(card.bottom + margin, window.innerHeight - box.height - margin);

  menu.style.left = `${Math.round(left)}px`;
  menu.style.top = `${Math.round(Math.max(margin, top))}px`;
}

// Clicking the pinned card or its text is not a dismissal; only the option
// buttons inside close the menu, and they do it themselves.
$("card-menu").addEventListener("click", (e) => e.stopPropagation());
// The cursor leaving the card to reach the menu must not close it.
$("card-menu").addEventListener("mouseenter", () => clearTimeout(closeTimer));
$("card-menu").addEventListener("mouseleave", unhoverMenu);
document.addEventListener("click", closeMenu);
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") closeMenu();
});

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

/// The trash is an open area (3-5-2) — either player may look through either
/// one — so this is a pile you can open, not a hidden zone.
function renderTrash(side, prefix, label) {
  const slot = $(`${prefix}-trash`);
  slot.innerHTML = "";

  const pile = document.createElement("div");
  pile.className = "trash-pile" + (side.trash.length === 0 ? " empty" : "");
  pile.title = `${label} trash — ${side.trash.length} card(s)`;

  if (side.trash.length > 0) {
    // Index 0 is the most recent card in (3-5-2), so it is the face-up top.
    pile.appendChild(cardEl(side.trash[0], { small: true, plain: true }));
    pile.addEventListener("click", () => openTrash(side, label));
  } else {
    pile.innerHTML = `<div class="trash-empty">trash</div>`;
  }

  const count = document.createElement("div");
  count.className = "trash-count";
  count.textContent = side.trash.length;
  pile.appendChild(count);
  slot.appendChild(pile);
}

function openTrash(side, label) {
  $("trash-title").textContent = `${label} trash`;
  $("trash-sub").textContent =
    `${side.trash.length} card(s), most recent first`;
  const grid = $("trash-grid");
  grid.innerHTML = "";
  for (const card of side.trash) grid.appendChild(cardEl(card, { plain: true }));
  $("trash-modal").hidden = false;
}

$("open-logs").addEventListener("click", async () => {
  try {
    await invoke("open_log_dir");
  } catch (err) {
    // Falling back to the path is enough to be useful.
    const dir = await invoke("log_dir").catch(() => null);
    $("setup-error").textContent = dir ? `Logs are in ${dir}` : String(err);
  }
});

$("show-debug").addEventListener("click", async () => {
  let info;
  try {
    info = await invoke("debug_info");
  } catch (err) {
    info = { path: null, withheld: String(err), entries: [], summary: [] };
  }

  $("debug-path").textContent = info.path
    ? `log: ${info.path}`
    : "no log for this session";

  $("debug-summary").innerHTML = (info.summary ?? [])
    .map(([k, v]) => `<div><span>${k}</span><b>${v}</b></div>`)
    .join("");

  const withheld = $("debug-withheld");
  withheld.hidden = !info.withheld;
  withheld.textContent = info.withheld ?? "";

  // Tail only: a long session runs to hundreds of records and the end is the
  // part anyone reads.
  $("debug-entries").textContent = (info.entries ?? []).slice(-60).join("\n");
  $("debug-modal").hidden = false;
});

$("debug-close").addEventListener("click", () => {
  $("debug-modal").hidden = true;
});

$("trash-close").addEventListener("click", () => {
  $("trash-modal").hidden = true;
});

$("choose-confirm").addEventListener("click", () => {
  if (lastSnapshot) submitChoice(picked, lastSnapshot);
});
$("choose-none").addEventListener("click", () => {
  if (lastSnapshot) submitChoice([], lastSnapshot);
});

function renderSide(view, side, prefix) {
  const yours = side === view.you;

  const leader = $(`${prefix}-leader`);
  leader.innerHTML = "";
  if (side.leader) leader.appendChild(cardSlot(side.leader, { yours }));

  const chars = $(`${prefix}-characters`);
  chars.innerHTML = "";
  if (side.characters.length === 0) {
    chars.innerHTML = `<div class="empty">no characters</div>`;
  }
  for (const c of side.characters) chars.appendChild(cardSlot(c, { yours }));

  const stage = $(`${prefix}-stage`);
  stage.innerHTML = "";
  if (side.stage) stage.appendChild(cardSlot(side.stage, { small: true, yours }));
}

function lifePips(n) {
  return `<span class="pips">${"●".repeat(n)}${"○".repeat(Math.max(0, 5 - n))}</span> ${n}`;
}

/** One action, as a button. Shared by the sidebar and the card menus so a
 *  given action looks and behaves the same wherever it is offered. */
function optionButton(opt) {
  const b = document.createElement("button");
  b.className = `opt ${opt.kind}`;
  b.textContent = opt.label;
  b.addEventListener("click", () => {
    closeMenu();
    choose(opt.index);
  });
  b.addEventListener("mouseenter", () => {
    highlighted = new Set(opt.cards);
    applyHighlight();
  });
  b.addEventListener("mouseleave", () => {
    highlighted = new Set();
    applyHighlight();
  });
  return b;
}

function fillOptions(container, options) {
  for (const opt of options) container.appendChild(optionButton(opt));
}

/** Whether the full list is expanded. Kept across renders: a disclosure that
 *  re-collapsed on every snapshot would be unusable during your own turn. */
let allOpen = false;

/** The actions that live on cards, flat and collapsed. A card menu is only
 *  findable if you guess the right card, so this stays as the index of them.
 *
 *  Card-bound only: the card-less actions are already listed above it, and
 *  showing the whole list here repeated every one of them. */
function allActions(options) {
  const box = document.createElement("details");
  box.className = "all-actions";
  box.open = allOpen;
  box.innerHTML = `<summary>Card actions (${options.length})</summary>`;
  box.addEventListener("toggle", () => {
    allOpen = box.open;
  });
  const list = document.createElement("div");
  list.className = "all-list";
  fillOptions(list, options);
  box.appendChild(list);
  return box;
}

// ---- choosing targets -------------------------------------------------------
//
// The engine offers a Choose as one action per subset, which is a list of
// combinations and unreadable past two candidates. This turns it back into
// what the player is actually doing: picking cards.

/** Cards picked so far, in click order. */
let picked = [];

const sameSet = (a, b) =>
  a.length === b.length && [...a].sort().every((v, i) => v === [...b].sort()[i]);

function renderChoose(snap) {
  const modal = $("choose-modal");
  if (snap.choose_up_to == null) {
    modal.hidden = true;
    picked = [];
    return;
  }

  const upTo = snap.choose_up_to;
  // Every candidate appears as a subset of one, so the singletons are the
  // candidate list — in the engine's order, which is the board's order.
  const candidates = [];
  for (const opt of snap.options) {
    if (opt.cards.length === 1 && !candidates.includes(opt.cards[0])) {
      candidates.push(opt.cards[0]);
    }
  }

  const index = cardIndex(snap.view);
  $("choose-title").textContent = snap.question ?? "Choose";
  $("choose-sub").textContent =
    upTo === 1
      ? "Pick a card."
      : `Pick up to ${upTo} — ${picked.length} chosen.`;

  const grid = $("choose-grid");
  grid.innerHTML = "";
  for (const id of candidates) {
    const card = index.get(id);
    const holder = document.createElement("div");
    holder.className = "choose-option" + (picked.includes(id) ? " picked" : "");

    if (card) {
      holder.appendChild(cardEl(card, { plain: true }));
    } else {
      // A candidate the view does not hold — off-board, mid-effect. The
      // engine's own label is the only description available for it.
      const opt = snap.options.find((o) => o.cards.length === 1 && o.cards[0] === id);
      holder.innerHTML = `<div class="choose-unknown">${opt ? opt.label : id}</div>`;
    }

    holder.appendChild(
      Object.assign(document.createElement("div"), {
        className: "choose-rank",
        textContent: picked.includes(id) ? String(picked.indexOf(id) + 1) : "",
      }),
    );

    holder.addEventListener("click", () => {
      if (upTo === 1) {
        submitChoice([id], snap);
        return;
      }
      if (picked.includes(id)) picked = picked.filter((p) => p !== id);
      else if (picked.length < upTo) picked.push(id);
      renderChoose(snap);
    });
    grid.appendChild(holder);
  }

  // Declining is only on the table when the engine actually offers it.
  const canDecline = snap.options.some((o) => o.cards.length === 0);
  $("choose-none").hidden = !canDecline;
  $("choose-confirm").hidden = upTo === 1;
  $("choose-confirm").disabled = picked.length === 0;
  modal.hidden = false;
}

/** Submits a set of cards by finding the option that names exactly them. */
function submitChoice(cards, snap) {
  const opt = snap.options.find((o) => sameSet(o.cards, cards));
  if (!opt) {
    $("question").textContent = "That combination is not on offer";
    return;
  }
  picked = [];
  $("choose-modal").hidden = true;
  choose(opt.index);
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

/** Whether `id` is a card the viewer controls, hand included. */
function isYours(view, id) {
  if (view.you.hand.some((c) => c.id === id)) return true;
  const { leader, stage, characters } = view.you;
  return [leader, stage, ...characters].some((c) => c && c.id === id);
}

/** Every card the viewer can click, for re-finding one across a re-render. */
function cardIndex(view) {
  const index = boardIndex(view);
  for (const c of view.you.hand) index.set(c.id, c);
  for (const side of [view.you, view.opponent]) {
    for (const c of side.trash) index.set(c.id, c);
  }
  return index;
}

const cardName = (c) => {
  if (!c) return "?";
  const info = c.number ? catalogue.get(c.number) : null;
  return info ? info.name : (c.number ?? "?");
};

function renderBattle(view) {
  const bar = $("battle-bar");
  const modal = $("battle-modal");

  if (!view.battle) {
    bar.hidden = true;
    modal.hidden = true;
    return;
  }

  const index = boardIndex(view);
  const attacker = index.get(view.battle.attacker);
  const defender = index.get(view.battle.target);
  const label = (c) =>
    c && c.power != null ? `${cardName(c)} <b>${c.power}</b>` : cardName(c);

  bar.hidden = false;
  bar.innerHTML = `
    <span class="atk">${label(attacker)}</span>
    <span class="arrow">&#8594;</span>
    <span class="def">${label(defender)}</span>
    <span class="step">${view.battle.step}</span>
  `;

  // The full view. Power shown here is derived, so DON!! and counters are
  // already folded in — which is the whole point of showing it during the
  // Counter step.
  modal.hidden = false;
  $("battle-step").textContent = `${view.battle.step} step`;

  for (const [slot, card] of [["attacker", attacker], ["defender", defender]]) {
    const holder = $(`battle-${slot}`);
    holder.innerHTML = "";
    if (card) holder.appendChild(cardEl(card, { large: true, plain: true }));
    $(`battle-${slot}-name`).textContent = cardName(card);
    $(`battle-${slot}-power`).textContent =
      card && card.power != null ? card.power : "—";
  }

  // 7-1-4-1: the attacker wins ties, so ">=" is the line that matters.
  const ap = attacker && attacker.power;
  const dp = defender && defender.power;
  const verdict = $("battle-verdict");
  if (typeof ap === "number" && typeof dp === "number") {
    const wins = ap >= dp;
    verdict.textContent = wins ? "attacker wins" : "attack repelled";
    verdict.className = `verdict ${wins ? "bad" : "good"}`;
  } else {
    verdict.textContent = "";
    verdict.className = "verdict";
  }
}

function render(snap) {
  const view = snap.view;
  battleAttacker = view.battle ? view.battle.attacker : null;
  battleDefender = view.battle ? view.battle.target : null;

  // Grouped before anything is drawn: a card needs to know whether it has
  // actions to decide whether it looks inert.
  //
  // A live battle keeps its decisions in the modal, which covers the board —
  // so no card carries a menu while one is running.
  // The Main Phase is the only decision whose actions belong to cards, so it
  // is the only one where having none means a card cannot act. During a
  // mulligan no card has actions and none should look inert.
  cardsCanAct = snap.pending_kind === "main" && !view.battle;

  menus = new Map();
  cardless = [];
  const carded = [];
  for (const opt of snap.options) {
    if (opt.subject == null || view.battle) {
      cardless.push(opt);
      continue;
    }
    carded.push(opt);
    const list = menus.get(opt.subject);
    if (list) list.push(opt);
    else menus.set(opt.subject, [opt]);
  }

  $("turn-label").textContent = snap.turn_label;
  $("session-id").textContent = snap.session_id ? `#${snap.session_id}` : "";
  $("session-id").title = "session id — matches the log filename";
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
  renderTrash(view.opponent, "opp", "Opponent");
  renderTrash(view.you, "you", "Your");
  renderDon(view.opponent, "opp", view.opponent.don_deck);
  renderDon(view.you, "you", view.you.don_deck);

  const hand = $("hand");
  hand.innerHTML = "";
  if (view.you.hand.length === 0) {
    hand.innerHTML = `<div class="empty">hand empty</div>`;
  }
  for (const c of view.you.hand) hand.appendChild(cardSlot(c, { yours: true }));

  renderBattle(view);
  renderChoose(snap);

  $("question").textContent =
    snap.question ?? (snap.thinking ? "Opponent is thinking…" : "Waiting…");

  const inBattle = !!view.battle;
  $("options").innerHTML = "";
  $("battle-options").innerHTML = "";
  $("battle-question").textContent = inBattle ? (snap.question ?? "") : "";

  if (inBattle) {
    // The modal owns the whole decision while a battle runs, so it gets the
    // undivided list.
    fillOptions($("battle-options"), snap.options);
  } else {
    fillOptions($("options"), cardless);
    if (carded.length > 0) {
      $("options").appendChild(allActions(carded));
    }
  }
  if (snap.thinking) {
    $(inBattle ? "battle-options" : "options").innerHTML =
      `<div class="thinking">Opponent is thinking…</div>`;
  }

  // The board was rebuilt underneath any open menu. A pinned one is re-anchored
  // to the new element, or dropped if that card has left play. An unpinned one
  // just closes: its card element is gone, so no mouseleave can ever arrive to
  // close it later, and hovering again costs nothing.
  if (selected !== null) {
    const el = document.querySelector(`.card[data-id="${selected}"]`);
    const card = cardIndex(view).get(selected);
    if (pinned && el && card && !inBattle) {
      el.classList.add("selected");
      openMenu(card, el, isYours(view, selected));
    } else {
      closeMenu();
    }
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
