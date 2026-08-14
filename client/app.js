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
/** The card the preview was pinned to by a click, which hover then cannot
 *  displace, or null. The menu is not pinned with it — the actions belong to
 *  whichever card the cursor is on, and reading one card while working another
 *  is the point of pinning at all. */
let pinnedId = null;
/** The card number the preview panel is showing, so an art fetch that resolves
 *  after the cursor has moved on cannot paint over the card that replaced it. */
let previewing = null;
/** Actions by subject id, plus the card-less remainder, from the last render. */
let menus = new Map();
let cardless = [];
/** Whether a card with no menu is genuinely unable to act right now. */
let cardsCanAct = false;
/** Hand cards present at the last render, to spot the ones that just arrived. */
let lastHandIds = new Set();
/** The order the hand is drawn in, oldest first, so arrivals appear on the
 *  right. The engine puts a draw at the back of the hand and a Life card at
 *  the front, which would make one appear on each side; hand order carries no
 *  rules meaning, so which end a card joins is the UI's to choose. */
let handOrder = [];
/** Life count per side last render, to spot cards that just left. */
const lastLifeCount = new Map();
/** Top card of each trash last render, to spot one that just landed. */
const lastTrashTop = new Map();
/** Why each trash's top card went there, for the pile's tooltip. */
const lastCause = new Map();
/** The cards in the battle currently being resolved, if any. */
let battleAttacker = null;
let battleDefender = null;
/** The attack being picked, or null: `{ attacker, targets }`, where `targets`
 *  maps a target's instance id to the option that attacks it. */
let attackPick = null;
/** The snapshot a picker or a staged activation was opened against, so a
 *  re-render of that same snapshot does not close one mid-decision. */
let openedFor = null;

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
 *
 * `preview` drops the corner panel too, which every overlay wants: the panel is
 * painted under them by design, so a hover wired to it from inside one is a
 * hover that does nothing. The battle modal has a second reason — it shows the
 * cards it is about at full size already.
 */
function cardEl(
  card,
  {
    small = false,
    large = false,
    yours = false,
    plain = false,
    preview = true,
    arriving = 0,
  } = {},
) {
  const el = document.createElement("div");
  el.className = "card" + (small ? " small" : "") + (large ? " large" : "");
  el.dataset.id = String(card.id);
  // A card that was not in hand last render grows into place, so a draw or a
  // Life card taken is visible as an arrival rather than the hand just being
  // one wider than it was.
  if (arriving) {
    el.classList.add("arriving");
    el.style.animationDelay = `${(arriving - 1) * 90}ms`;
  }
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
    // No menu here, but the corner panel is how every card is read — except
    // where the surrounding overlay is already showing the card.
    if (preview) {
      el.addEventListener("mouseenter", () => showPreview(card));
      el.addEventListener("mouseleave", hidePreview);
    }
    return el;
  }

  // Keyed to the decision, not to whether any card happens to have a menu:
  // with no DON!! left, nothing is playable and every card should dim, which
  // is exactly when a "some card has a menu" test switches the dimming off.
  if (cardsCanAct && yours && !menus.has(card.id)) {
    el.classList.add("inert");
  }
  if (card.id === selected) el.classList.add("selected");
  // Which card the pinned preview is of, which several copies of one card in
  // play makes impossible to read off the panel itself.
  if (card.id === pinnedId) el.classList.add("pinned");

  el.addEventListener("mouseenter", () => hoverMenu(card, el));
  el.addEventListener("mouseleave", unhoverMenu);
  el.addEventListener("click", (e) => {
    e.stopPropagation();
    pinMenu(card, el);
  });
  return el;
}

// ---- the card preview -------------------------------------------------------
//
// The board's top-right corner, and the one place a card is read: hovering any
// card puts it there, and the loupe over it magnifies further. It follows the
// cursor off the card like the menu does — a panel that stayed would be a panel
// showing a card you are no longer looking at.
//
// Clicking pins it, which is the only route to the loupe: while it is still
// following the cursor the panel takes no pointer events at all, so that it
// cannot swallow the board it is sitting on.
//
// Only the preview pins. The menu goes on following the cursor, so a pinned
// card can be read while the card being played is the one under it.

/** Shows `card` — a VisibleCard — in the corner panel. */
async function showPreview(card) {
  // A pinned preview is what hovering another card is not allowed to displace.
  if (pinnedId !== null && card?.id !== pinnedId) return;

  const box = $("preview");
  // A card the viewer may not identify has nothing to show, and leaving the
  // last one up would credit it to the card under the cursor. `cardEl` returns
  // before wiring hover or click onto one, so this is a guard rather than a
  // path — but a pin the panel cannot draw is a pin nothing can click off, so
  // it releases rather than just hiding.
  if (!card || !card.number) {
    unpinPreview();
    previewing = null;
    box.innerHTML = "";
    box.hidden = true;
    return;
  }

  const number = card.number;
  const info = catalogue.get(number);
  previewing = number;
  const uri = await art(number);

  // The cursor moved on while the art resolved.
  if (previewing !== number) return;

  // A Character given DON!! is exactly the card a player stops to read, and its
  // printed power is then the one number on screen that is wrong — so the meta
  // line carries what the card is at now, marked as such.
  const printed = info ? info.power : null;
  const power = card.power ?? printed;
  const powerText = power == null ? "" : ` · ${power}${power === printed ? "" : " now"}`;

  box.innerHTML = `
    ${uri ? `<img src="${uri}" alt="${number}" />` : ""}
    <div class="ptext">
      <div class="pname">${info ? info.name : number}</div>
      <div class="pmeta">${info ? `${number} · ${info.category} · cost ${info.cost}${powerText}${
        info.counter != null ? ` · counter ${info.counter}` : ""
      }` : number}</div>
      ${info && info.effect ? `<div class="peffect">${info.effect.replaceAll("<br>", "<br/>")}</div>` : ""}
      ${info && info.trigger ? `<div class="ptrigger">${info.trigger}</div>` : ""}
    </div>
  `;
  box.classList.toggle("pinned", pinnedId !== null);
  box.hidden = false;
}

/** Clears the panel, unless it is pinned — which is what a pin is. */
function hidePreview() {
  if (pinnedId !== null) return;
  previewing = null;
  const box = $("preview");
  box.hidden = true;
  box.innerHTML = "";
  // The preview can go while the cursor is still over where it was, and the
  // loupe must not outlive the image it is magnifying.
  hideLoupe();
}

/** Releases the pin and takes the panel with it. */
function unpinPreview() {
  if (pinnedId === null) return;
  for (const c of document.querySelectorAll(".card.pinned")) {
    c.classList.remove("pinned");
  }
  pinnedId = null;
  $("preview").classList.remove("pinned");
  hidePreview();
}

// ---- card menu --------------------------------------------------------------
//
// The actions that start from a card, above the card itself. The card is read
// in the corner panel, which the same hover opens; this side of it is nothing
// but buttons. The flat list is still in the sidebar, so nothing here is the
// only route to a legal action.
//
// Both edges are delayed. Opening waits so that sweeping the cursor across a
// row does not fire a menu per card; closing waits because the cursor has to
// cross a gap to reach the menu, and a menu that vanishes on the way is one
// you cannot click.

const OPEN_DELAY = 90;
const CLOSE_DELAY = 180;
let openTimer = null;
let closeTimer = null;

function hoverMenu(card, el) {
  clearTimeout(closeTimer);
  if (selected === card.id) return;
  clearTimeout(openTimer);
  openTimer = setTimeout(() => showMenu(card, el), OPEN_DELAY);
}

function unhoverMenu() {
  clearTimeout(openTimer);
  closeTimer = setTimeout(closeMenu, CLOSE_DELAY);
}

/** A click pins the preview where hover would have let it go. Clicking the
 *  pinned card again releases it; the menu is untouched either way, and goes on
 *  following the cursor. */
function pinMenu(card, el) {
  clearTimeout(openTimer);
  clearTimeout(closeTimer);
  if (card.id === pinnedId) {
    unpinPreview();
    return;
  }
  // Moving the pin rather than clearing it: the panel is about to show this
  // card, and taking it down on the way would only flicker.
  for (const c of document.querySelectorAll(".card.pinned")) {
    c.classList.remove("pinned");
  }
  pinnedId = card.id;
  showMenu(card, el);
}

function showMenu(card, el) {
  selected = card.id;
  for (const c of document.querySelectorAll(".card.selected")) {
    c.classList.remove("selected");
  }
  // The outline marks the card the menu belongs to, so it is worth drawing on
  // one that has no actions at all: it says the hover was noticed.
  el.classList.add("selected");
  if (card.id === pinnedId) el.classList.add("pinned");
  showPreview(card);
  openMenu(card, el);
}

/** Drops the actions and the outline, and the preview with them unless it was
 *  pinned. */
function closeMenu() {
  clearTimeout(openTimer);
  clearTimeout(closeTimer);
  selected = null;
  $("card-menu").hidden = true;
  hidePreview();
  for (const c of document.querySelectorAll(".card.selected")) {
    c.classList.remove("selected");
  }
}

function openMenu(card, el) {
  const menu = $("card-menu");
  const options = menus.get(card.id) ?? [];

  // With the card in the corner panel there is nothing left to put above a card
  // that cannot act, and an empty box over every card the cursor crosses is
  // noise. A card of yours with no actions is already dimmed, which says it.
  if (options.length === 0) {
    menu.hidden = true;
    return;
  }

  menu.innerHTML = `<div class="menu-actions"></div>`;
  const list = menu.querySelector(".menu-actions");
  // Every attack this card can make becomes one button; the rest are per-action
  // as before.
  const attacks = options.filter((o) => o.kind === "attack");
  if (attacks.length > 0) list.appendChild(attackButton(card.id, attacks));
  fillOptions(list, options.filter((o) => o.kind !== "attack"));

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

// A click inside the menu is not a dismissal; only the option buttons close it,
// and they do it themselves.
//
// Load-bearing for Attack, too: without it the click that opens the target
// picker would go on to reach the document listener below and close it again
// on its way out, leaving Attack looking like a button that does nothing.
$("card-menu").addEventListener("click", (e) => e.stopPropagation());
// The cursor leaving the card to reach the menu must not close it.
$("card-menu").addEventListener("mouseenter", () => clearTimeout(closeTimer));
$("card-menu").addEventListener("mouseleave", unhoverMenu);

// These reach the panel only while it is pinned: unpinned it is
// `pointer-events: none`, so that it cannot trap the opponent's trash pile
// underneath it. Pinning is therefore the way in to both the loupe and the zoom.
//
// The preview is not a control, so a click on it is free to mean "let me
// actually read this" — and must not reach the document, which would take it
// for a miss and clear the very panel that was clicked.
$("preview").addEventListener("click", (e) => {
  e.stopPropagation();
  const img = e.target.closest("#preview img");
  if (img) openZoom(img.src, img.alt);
});
// A menu belonging to some other card stays open while the cursor is in here,
// the same way it does over the menu itself.
$("preview").addEventListener("mouseenter", () => clearTimeout(closeTimer));
$("preview").addEventListener("mouseleave", () => {
  hideLoupe();
  unhoverMenu();
});

// ---- loupe ------------------------------------------------------------------
//
// The preview's card is the largest one the board has room beside it for, and
// on a small window that is still short of readable for printed effect text.
// Magnifying it under the cursor is the rest of the way there.

/** Half the loupe, and how far it sits from the cursor. From the CSS. */
const LOUPE_W = 320;
const LOUPE_H = 220;
const LOUPE_GAP = 20;

/** Floor on the magnification, for a window wide enough that the preview's card
 *  is already close to the art's own 600px. Past native this is an upscale,
 *  but a soft big glyph still beats a sharp small one. */
const LOUPE_MIN_ZOOM = 1.8;

function moveLoupe(e) {
  const img = e.target.closest("#preview img");
  if (!img || !img.naturalWidth) return hideLoupe();

  const rect = img.getBoundingClientRect();
  const loupe = $("loupe");
  const zoom = Math.max(LOUPE_MIN_ZOOM, img.naturalWidth / rect.width);
  const zoomed = { w: rect.width * zoom, h: rect.height * zoom };

  // The point under the cursor, placed at the middle of the loupe — then held
  // inside the image, so approaching an edge slides the view rather than
  // opening a gap of background beside the card.
  const at = (cursor, start, span, zoomedSpan, box) => {
    const ratio = (cursor - start) / span;
    return Math.min(0, Math.max(box - zoomedSpan, box / 2 - ratio * zoomedSpan));
  };
  const x = at(e.clientX, rect.left, rect.width, zoomed.w, LOUPE_W);
  const y = at(e.clientY, rect.top, rect.height, zoomed.h, LOUPE_H);

  loupe.style.backgroundImage = `url("${img.src}")`;
  loupe.style.backgroundSize = `${zoomed.w}px ${zoomed.h}px`;
  loupe.style.backgroundPosition = `${Math.round(x)}px ${Math.round(y)}px`;

  // Beside the cursor, and flipped rather than clipped near an edge.
  let left = e.clientX + LOUPE_GAP;
  if (left + LOUPE_W > window.innerWidth) left = e.clientX - LOUPE_GAP - LOUPE_W;
  let top = e.clientY + LOUPE_GAP;
  if (top + LOUPE_H > window.innerHeight) top = e.clientY - LOUPE_GAP - LOUPE_H;
  loupe.style.left = `${Math.round(Math.max(0, left))}px`;
  loupe.style.top = `${Math.round(Math.max(0, top))}px`;
  loupe.hidden = false;
}

function hideLoupe() {
  $("loupe").hidden = true;
}

// ---- the card at full size --------------------------------------------------

function openZoom(src, alt) {
  const box = $("card-zoom");
  box.innerHTML = `<img src="${src}" alt="${alt ?? ""}" />`;
  box.hidden = false;
  // The cursor is over the preview, not the card it just opened.
  hideLoupe();
}

function closeZoom() {
  const box = $("card-zoom");
  box.hidden = true;
  box.innerHTML = "";
}

const zoomOpen = () => !$("card-zoom").hidden;

// Anywhere on the backdrop, including the card: there is nothing to do in here
// but look, so every click is a dismissal. Held back from the document, which
// would otherwise take the same click as a miss and close the menu behind it.
$("card-zoom").addEventListener("click", (e) => {
  e.stopPropagation();
  closeZoom();
});

$("preview").addEventListener("mousemove", moveLoupe);
// Rebuilding the preview under a still cursor leaves the loupe showing the card
// that was there before.
$("preview").addEventListener("mouseover", moveLoupe);
// A click that reaches the document missed every card, so it is a miss in both
// senses: it dismisses the menu and the preview, and the picker's backdrop is
// nothing but a miss.
document.addEventListener("click", () => {
  closeAttackPicker();
  unpinPreview();
  closeMenu();
});
// Not on the document handler above: an activation is staged *by* a click, and
// that click is still on its way out when the handler runs.
document.addEventListener("keydown", (e) => {
  if (e.key !== "Escape") return;
  // Escape backs out one step at a time, outermost first: the card being read
  // covers everything, then the Choose, then the two pickers, then the menu and
  // the preview underneath them.
  if (zoomOpen()) closeZoom();
  else if (chooseOpen()) dismissChoose();
  else if (staged) cancelActivation();
  else if (attackPick) closeAttackPicker();
  else {
    unpinPreview();
    closeMenu();
  }
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
      // Fan sideways so each given DON!! is individually countable. The step
      // is in the stylesheet, next to the width it has to keep pace with.
      d.style.setProperty("--i", String(i));
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

  // The DON!! deck is its own zone on the mat, not the tail of the cost area.
  const slot = $(`${prefix}-don-deck`);
  slot.innerHTML = "";
  const remaining = document.createElement("div");
  remaining.className = "don-deck";
  remaining.title = "DON!! deck";
  remaining.textContent = deckCount;
  slot.appendChild(remaining);
}

/// The trash is an open area (3-5-2) — either player may look through either
/// one — so this is a pile you can open, not a hidden zone.
function renderTrash(side, prefix, label) {
  const slot = $(`${prefix}-trash`);
  slot.innerHTML = "";

  const pile = document.createElement("div");
  pile.className = "trash-pile" + (side.trash.length === 0 ? " empty" : "");

  // Why the top card is there. The pile is the only place left that can say
  // so, now that nothing flies across the board carrying the reason.
  const cause = lastCause.get(prefix);
  pile.title =
    `${label} trash — ${side.trash.length} card(s)` +
    (cause ? `\nmost recent: ${cause}` : "");

  if (side.trash.length > 0) {
    // Index 0 is the most recent card in (3-5-2), so it is the face-up top.
    const top = side.trash[0];
    const isNew = lastTrashTop.get(prefix) !== top.id;
    lastTrashTop.set(prefix, top.id);
    pile.appendChild(
      cardEl(top, { small: true, plain: true, arriving: isNew ? 1 : 0 }),
    );
    pile.addEventListener("click", () => openTrash(side, label));
  } else {
    lastTrashTop.set(prefix, null);
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
  for (const card of side.trash) {
    grid.appendChild(cardEl(card, { plain: true, preview: false }));
  }
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

/** The Life area, as face-down cards.
 *
 * Contents are secret to *both* players (3-1-4), so there is nothing to draw
 * but backs — which is also what the area looks like on a table. */
function renderLife(side, prefix) {
  const box = $(`${prefix}-life-cards`);
  box.innerHTML = "";
  box.classList.toggle("none", side.life_count === 0);

  // Counted rather than identified: Life is a count in the view, so there are
  // no ids to diff. A drop is the only signal that a card left, and it fires
  // once because the count then stays put until the next one goes.
  const before = lastLifeCount.get(prefix);
  const lost = before == null ? 0 : Math.max(0, before - side.life_count);
  lastLifeCount.set(prefix, side.life_count);

  // Overlapped by CSS margin rather than absolute offsets, so the pile sizes
  // itself and stays inside its zone however many cards are in it.
  for (let i = 0; i < side.life_count; i++) {
    const c = document.createElement("div");
    c.className = "card facedown life-card";
    c.style.zIndex = String(i);
    c.innerHTML = `<div class="back"></div>`;
    box.appendChild(c);
  }

  // The cards that just went, flashing where they were before fading. Left in
  // the flow so the pile does not close the gap until they are gone.
  for (let i = 0; i < lost; i++) {
    const ghost = document.createElement("div");
    ghost.className = "card facedown life-card leaving";
    ghost.style.zIndex = String(side.life_count + i);
    ghost.style.animationDelay = `${i * 160}ms`;
    ghost.innerHTML = `<div class="back"></div>`;
    ghost.addEventListener("animationend", () => ghost.remove());
    box.appendChild(ghost);
  }

  // Counted like the deck and the trash, so the three piles read alike — and
  // Life is the one whose number decides the game.
  const count = document.createElement("div");
  count.className = "trash-count life-count";
  count.textContent = side.life_count;
  box.appendChild(count);
}

/** The deck, as a face-down pile with its count. */
function renderDeck(side, prefix) {
  const slot = $(`${prefix}-deck-pile`);
  slot.innerHTML = "";

  const pile = document.createElement("div");
  pile.className = "pile" + (side.deck_count === 0 ? " empty" : "");
  pile.title = `deck — ${side.deck_count} card(s)`;

  if (side.deck_count > 0) {
    const c = document.createElement("div");
    c.className = "card small facedown";
    c.innerHTML = `<div class="back"></div>`;
    pile.appendChild(c);
  } else {
    pile.innerHTML = `<div class="trash-empty">deck</div>`;
  }

  const count = document.createElement("div");
  count.className = "trash-count";
  count.textContent = side.deck_count;
  pile.appendChild(count);
  slot.appendChild(pile);
}

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


/** One action, as a button. Shared by the sidebar and the card menus so a
 *  given action looks and behaves the same wherever it is offered. */
function optionButton(opt) {
  const b = document.createElement("button");
  b.className = `opt ${opt.kind}`;
  b.textContent = opt.label;
  b.addEventListener("click", () => {
    closeMenu();
    // An activation is the one action with nothing behind it: 8-3-1-4 gives the
    // controller the refusal of an *auto* effect's cost, and the engine is
    // explicit that activating is itself the agreement, so once this is sent
    // there is no move that takes it back. Staging it is the refusal the rules
    // do not provide, held on this side of the wire.
    if (opt.kind === "effect") stageActivation(opt);
    else choose(opt.index);
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

// ---- staging an activation --------------------------------------------------
//
// Every other misclick on this board is recoverable: a menu closes, a target
// picker cancels, a Choose can be waved away. Activating an ability is the one
// that is not — the engine takes it as the agreement and resolves it — so the
// place to put the second thoughts is before it is sent.
//
// Deliberately not in the engine. A staged action is a fact about a player
// hesitating, not about the game, and `GameState` is a pure function of its
// actions; a state that exists only until someone clicks would have to be
// serialised, replayed and searched over by MCTS for no gain.

/** The activation awaiting confirmation, or null. Holds an option index, so it
 *  cannot outlive the snapshot it was read from. */
let staged = null;

function stageActivation(opt) {
  const snap = lastSnapshot;
  if (!snap) return;

  staged = opt;
  openedFor = snap;
  const card = opt.subject != null ? cardIndex(snap.view).get(opt.subject) : null;
  const info = card && card.number ? catalogue.get(card.number) : null;

  $("activate-title").textContent = card ? `Activate ${cardName(card)}?` : "Activate?";
  $("activate-sub").textContent = opt.label;
  renderActivateSource(card, info);
  renderActivateWarning(opt);
  const picking = renderActivateTargets(opt, snap);

  // Sending without picking here stays reachable, and is never hidden: it does
  // not answer the choice, it leaves it to the engine's own modal — where the
  // full pool is listed, several cards can be taken, and declining an "up to"
  // (8-4-4-1) has its own button. Cancel is the other thing entirely, and
  // abandons the activation.
  const confirm = $("activate-confirm");
  confirm.hidden = false;
  confirm.textContent = opt.warning
    ? "Activate anyway"
    : picking
      ? "Activate, choose afterwards"
      : "Activate";
  $("activate-modal").hidden = false;
}

/** The first choice, asked before the activation is sent.
 *
 * The attack picker's bargain, applied to abilities: nothing has been committed
 * while this is on screen, so Cancel costs nothing. Picking sends the
 * activation and the answer together.
 */
function renderActivateTargets(opt, snap) {
  const box = $("activate-targets");
  const preview = opt.targets;

  // Three reasons not to pick here, all of which leave the real decision to
  // the engine's own Choose: nothing to choose between; a secret pool, whose
  // contents are not ours to show before the cost that buys them; and a choice
  // of more than one card, which a single click cannot express.
  if (!preview || preview.secret || !preview.single || preview.cards.length === 0) {
    box.hidden = true;
    return false;
  }

  $("activate-targets-label").textContent =
    preview.cards.length === 1 ? "Choose its target" : `Choose one of ${preview.cards.length}`;

  const index = cardIndex(snap.view);
  const grid = $("activate-grid");
  grid.innerHTML = "";
  for (const { id, label } of preview.cards) {
    const card = index.get(id);
    const holder = document.createElement("div");
    holder.className = "choose-option";
    // A DON!! in the cost area is not on the board the view publishes and has
    // no art to draw, so its label is the only description there is.
    if (card) holder.appendChild(cardEl(card, { plain: true, preview: false }));
    else holder.innerHTML = `<div class="choose-unknown">${label}</div>`;
    holder.addEventListener("click", () => commitActivation(id));
    grid.appendChild(holder);
  }
  box.hidden = false;
  return true;
}

/** Whether this activation is about to do nothing, and what it wanted.
 *
 * The whole point of staging. `[Once Per Turn]` is spent by activating, not by
 * resolving, so a player who learns the pool is empty when the empty choice
 * appears has learned it one action too late.
 */
function renderActivateWarning(opt) {
  const box = $("activate-warning");
  // Silence when the effect has everything it needs. A panel that always says
  // something is a panel nobody reads by the third turn.
  if (!opt.warning) {
    box.hidden = true;
    return;
  }
  box.className = "notice bad";
  box.textContent = `${opt.warning} Activating spends the ability and changes nothing.`;
  box.hidden = false;
}

/** The card being activated, with the text that is about to happen. */
async function renderActivateSource(card, info) {
  const box = $("activate-source");
  if (!card) {
    box.hidden = true;
    return;
  }

  // Hidden for the round trip, not left showing the last card staged. The
  // modal is revealed synchronously, so anything still in here belongs to a
  // different card and would sit under this one's title until the art lands.
  box.hidden = true;

  // The staging this render belongs to. Art resolves out of order — a cached
  // card returns on a microtask while an uncached one is still on the wire —
  // so without this, cancelling one card and staging another can leave the
  // first one's art and text under the second one's title.
  const mine = staged;
  const uri = await art(card.number);
  if (staged !== mine) return;
  box.innerHTML = `
    <div class="source-art">${uri ? `<img src="${uri}" alt="${card.number}" />` : (card.number ?? "")}</div>
    <div class="source-text">
      <div class="source-who" title="${card.number ?? ""}">${info ? info.name : card.number}</div>
      ${info && info.effect ? `<div class="peffect">${info.effect.replaceAll("<br>", "<br/>")}</div>` : ""}
    </div>
  `;
  box.hidden = false;
}

function cancelActivation() {
  if (!staged) return;
  staged = null;
  $("activate-modal").hidden = true;
}

/** Sends the activation, and the target with it when one was picked.
 *
 * Two actions, because that is what the engine takes: the activation, then the
 * answer to the question it raises. `choose` resolves once the first has been
 * applied, so the snapshot waiting afterwards is the one holding that question.
 *
 * If the engine ends up offering something narrower than the pool this was
 * picked from — a cost paid in between can do that — `submitChoice` declines to
 * guess and leaves the Choose on screen, which is the right place for a
 * decision that turned out to still be open.
 */
async function commitActivation(target) {
  if (!staged) return;
  const opt = staged;
  cancelActivation();

  await choose(opt.index);

  if (target == null) return;
  const snap = lastSnapshot;
  // Only the effect's own choice, which `pending_kind` is the way to tell. An
  // activation whose cost is DON!! −X raises the return decision *first*, from
  // `pay`, and that one also fills `choose_up_to` — answering it with a card
  // the effect wanted matches no option and buries the real prompt under "that
  // combination is not on offer".
  if (!snap || snap.pending_kind !== "choose") return;
  // And the choice actually on offer, not merely a choice. `DigTop` suspends
  // with a `Choose` too, so an effect that digs before it asks would otherwise
  // have this answered against the dig. Leaving it unanswered puts the real
  // question on screen, which is where an open decision belongs.
  if (!(snap.choose_candidates ?? []).some((c) => c.id === target)) return;
  submitChoice([target], snap);
}

$("activate-confirm").addEventListener("click", () => commitActivation());
$("activate-cancel").addEventListener("click", cancelActivation);
// The backdrop is everything the panel is not, so a click landing on the
// overlay itself is a click on nothing.
$("activate-modal").addEventListener("click", (e) => {
  if (e.target === $("activate-modal")) cancelActivation();
});

// ---- picking what to attack -------------------------------------------------
//
// The engine offers an attack as one action per (attacker, target) pair, so a
// filled board is a dozen near-identical buttons reading "Attack X into Y". This
// is the same collapse `renderChoose` does below for Choose, for the same
// reason: turn a list of combinations back into what the player is doing, which
// is picking an attacker and then picking what it hits.
//
// The second half is a modal built like the engine's own Choose, so "pick a
// card" looks the same wherever the question comes from. It replaced aiming on
// the board, which asked a player to notice that a mode had started and that
// some cards had grown outlines — and a tester who could not tell what was
// selectable never found it.
//
// The eligible targets are read off the options the engine sent. They are never
// recomputed from the rules here — a second opinion in JS about what may be
// attacked is exactly the divergence this must not introduce.

/** The distinct targets among an attacker's options, each mapped to the option
 *  that hits it. `Action::Attack` sends `[attacker, target]` as its cards.
 *
 *  One attacker reaches a given target exactly one way today, so the first-wins
 *  rule below never discards anything. If that stops being true — a second
 *  attack on the same target, differing in something the pair does not name —
 *  the board would offer only the first and the flat list would hold both. That
 *  is the intended degradation rather than an oversight: the list is documented
 *  as the complete index precisely so a collapse here cannot strand an action.
 */
function attackTargets(attacks) {
  const targets = new Map();
  for (const opt of attacks) {
    const target = opt.cards[1];
    if (target != null && !targets.has(target)) targets.set(target, opt);
  }
  return targets;
}

/** The one Attack button that stands in for all of them. */
function attackButton(attackerId, attacks) {
  const targets = attackTargets(attacks);

  const b = document.createElement("button");
  b.className = "opt attack";
  b.textContent = "Attack";
  b.addEventListener("click", () => openAttackPicker(attackerId, targets));
  // Hover previews the reach without entering the mode, so "what can this
  // hit?" does not cost a click in and a click back out.
  b.addEventListener("mouseenter", () => {
    highlighted = new Set([attackerId, ...targets.keys()]);
    applyHighlight();
  });
  b.addEventListener("mouseleave", () => {
    highlighted = new Set();
    applyHighlight();
  });
  return b;
}

/** The attacker, with its current power, above the targets it is choosing
 *  between. The comparison is the whole decision, and the one a new player is
 *  least equipped to make from memory. */
async function renderAttackSource(card) {
  const box = $("attack-source");
  if (!card) {
    box.hidden = true;
    return;
  }

  const info = card.number ? catalogue.get(card.number) : null;
  const uri = await art(card.number);
  box.innerHTML = `
    <div class="source-art">${uri ? `<img src="${uri}" alt="${card.number}" />` : (card.number ?? "")}</div>
    <div class="source-text">
      <div class="source-who" title="${card.number ?? ""}">${
        info ? info.name : (card.number ?? "?")
      } attacks${card.power != null ? ` at ${card.power}` : ""}</div>
      ${info && info.effect ? `<div class="peffect">${info.effect.replaceAll("<br>", "<br/>")}</div>` : ""}
    </div>
  `;
  box.hidden = false;
}

function openAttackPicker(attackerId, targets) {
  const snap = lastSnapshot;
  if (!snap) return;

  const index = cardIndex(snap.view);
  attackPick = { attacker: attackerId, targets };
  openedFor = snap;
  highlighted = new Set();
  closeMenu();
  applyHighlight();

  const attacker = index.get(attackerId);
  $("attack-title").textContent = `Attack with ${cardName(attacker)}`;
  $("attack-sub").textContent =
    targets.size === 1 ? "One target." : `Pick a target — ${targets.size} eligible.`;
  renderAttackSource(attacker);

  const grid = $("attack-grid");
  grid.innerHTML = "";
  for (const [id, opt] of targets) {
    const card = index.get(id);
    const holder = document.createElement("div");
    holder.className = "choose-option";

    // Each target draws itself, power badge and all, so the comparison the
    // attack turns on (7-1-4-1) is on screen rather than remembered.
    if (card) holder.appendChild(cardEl(card, { plain: true, preview: false }));
    else holder.innerHTML = `<div class="choose-unknown">${opt.label}</div>`;

    holder.addEventListener("click", () => {
      closeAttackPicker();
      choose(opt.index);
    });
    grid.appendChild(holder);
  }

  $("attack-modal").hidden = false;
}

function closeAttackPicker() {
  if (!attackPick) return;
  attackPick = null;
  $("attack-modal").hidden = true;
}

$("attack-cancel").addEventListener("click", closeAttackPicker);

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

/** The snapshot whose Choose the player has waved away, or null.
 *
 * Keyed by snapshot rather than a bare flag because one snapshot is rendered
 * more than once — the trash animation re-renders the same one — and a flag
 * would let the modal spring back while the board was being looked at.
 *
 * Dismissing is only ever visual. Escape must not answer for the player: the
 * empty answer is a real move that resolves the effect for nothing, and it
 * stays where it was, behind a button that says so. What was owed is still
 * owed, and the flat list in the sidebar still holds every option.
 */
let chooseDismissedFor = null;

const sameSet = (a, b) =>
  a.length === b.length && [...a].sort().every((v, i) => v === [...b].sort()[i]);

/** The card whose effect is asking, with its text.
 *
 * "Choose up to 1" says nothing about why, and by the time the prompt appears
 * the source may be one of several cards that could have produced it. */
async function renderChooseSource(number) {
  const box = $("choose-source");
  if (!number) {
    box.hidden = true;
    return;
  }

  const info = catalogue.get(number);
  const uri = await art(number);
  box.innerHTML = `
    <div class="source-art">${uri ? `<img src="${uri}" alt="${number}" />` : number}</div>
    <div class="source-text">
      <div class="source-who" title="${number ?? ""}">${info ? info.name : number} is asking</div>
      ${info && info.effect ? `<div class="peffect">${info.effect.replaceAll("<br>", "<br/>")}</div>` : ""}
      ${info && info.trigger ? `<div class="ptrigger">${info.trigger}</div>` : ""}
    </div>
  `;
  box.hidden = false;
}

function renderChoose(snap) {
  const modal = $("choose-modal");
  if (snap.choose_up_to == null) {
    modal.hidden = true;
    picked = [];
    return;
  }
  if (chooseDismissedFor === snap) {
    modal.hidden = true;
    return;
  }

  const upTo = snap.choose_up_to;
  const atLeast = snap.choose_at_least ?? 0;
  // The whole pool, sent by the engine side with a label and a class each:
  // the board cannot draw a given DON!!, and cannot tell which picks are
  // equivalent.
  const candidates = snap.choose_candidates ?? [];

  const index = cardIndex(snap.view);
  // Declining is only on the table when the engine actually offers it — an
  // "up to" (8-4-4-1) does, a fixed count does not.
  const canDecline = snap.options.some((o) => o.cards.length === 0);

  $("choose-title").textContent = snap.question ?? "Choose";
  renderChooseSource(snap.choose_source);
  const asked =
    upTo === 1
      ? "Pick a card."
      : atLeast === upTo
        ? `Pick ${upTo} — ${picked.length} chosen.`
        : `Pick up to ${upTo} — ${picked.length} chosen.`;
  // Said out loud, because the alternative is a player hunting the panel for a
  // way out that the rules do not have.
  $("choose-sub").textContent = canDecline
    ? asked
    : `${asked} This one cannot be declined.`;

  const grid = $("choose-grid");
  grid.innerHTML = "";
  for (const { id, label } of candidates) {
    const card = index.get(id);
    const holder = document.createElement("div");
    holder.className = "choose-option" + (picked.includes(id) ? " picked" : "");

    if (card) {
      holder.appendChild(cardEl(card, { plain: true, preview: false }));
    } else {
      // A candidate the view does not hold — a DON!!, or something off-board
      // mid-effect. Its label is the only description available for it.
      holder.innerHTML = `<div class="choose-unknown">${label}</div>`;
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

  $("choose-none").hidden = !canDecline;
  $("choose-confirm").hidden = upTo === 1;
  // Enabled only on an answer the engine will accept: a floor of 0 needs one
  // card, a fixed count needs exactly that many.
  $("choose-confirm").disabled = picked.length < Math.max(atLeast, 1);
  modal.hidden = animating();
}

/** Puts the Choose aside so the board can be read.
 *
 * Not an answer to it. Declining is a move the engine has to offer — it only
 * exists for an "up to" (8-4-4-1) — and it has its own button, which says what
 * it does.
 */
function dismissChoose() {
  if (!lastSnapshot || lastSnapshot.choose_up_to == null) return;
  chooseDismissedFor = lastSnapshot;
  // Through `render`, not `renderChoose`: the way back into the modal is a
  // button in the sidebar, and the sidebar is the rest of this function.
  render(lastSnapshot);
}

const chooseOpen = () => !$("choose-modal").hidden;

// The backdrop is the part of the overlay that is not the panel, so a click
// landing on the overlay itself missed everything there was to click.
$("choose-modal").addEventListener("click", (e) => {
  if (e.target === $("choose-modal")) dismissChoose();
});

/** The multiset of classes a pick covers, as a sorted key. */
function classKey(ids, candidates) {
  return ids
    .map((id) => candidates.find((c) => c.id === id)?.class ?? String(id))
    .sort()
    .join("|");
}

/**
 * Submits a set of cards by finding the option that names them.
 *
 * Falls back to matching by class, because the engine offers one representative
 * per class for interchangeable cards: picking the other of two identical
 * rested DON!! is the same answer, and must not be refused.
 */
// ---- arranging the top of the deck -----------------------------------------
//
// "Return them to the top or bottom of the deck in any order" is offered by the
// engine as one action per arrangement — 24 of them for three cards, every one
// reading "Top: … · Bottom: …". That is the same unreadable list of
// combinations `renderChoose` exists to collapse, and the same answer applies:
// let the player build the arrangement, then find the action that matches it.
//
// Placement order is the point, so this is not a set-picker. A card goes to the
// top or the bottom, and where it lands within that pile depends on when it was
// placed — first onto the top pile ends up topmost, first onto the bottom pile
// ends up highest of the buried ones.

/** Placements so far, in the order they were made: `{ id, pile }`.
 *
 *  One list rather than two, so undo has something to pop: with a top pile and
 *  a bottom pile there is no way to tell which of them moved last.
 */
let arrangePlacements = [];

const arrangePile = (pile) =>
  arrangePlacements.filter((p) => p.pile === pile).map((p) => p.id);

function renderArrange(snap) {
  const modal = $("arrange-modal");
  if (!snap.arrange) {
    modal.hidden = true;
    arrangePlacements = [];
    return;
  }

  const arrangeTop = arrangePile("top");
  const arrangeBottom = arrangePile("bottom");
  // The cards come with the decision rather than being looked up on the board:
  // they are lifted out of the deck into no area at all while the question is
  // pending, so `snap.view` does not hold them and never will.
  const byId = new Map(snap.arrange.map((c) => [c.id, c]));
  const all = snap.arrange.map((c) => c.id);
  const placed = arrangePlacements.map((p) => p.id);
  const remaining = all.filter((id) => !placed.includes(id));
  const describe = (id) => cardEl(byId.get(id), { plain: true, preview: false });

  $("arrange-title").textContent = snap.question ?? "Arrange";
  $("arrange-sub").textContent = remaining.length
    ? `${remaining.length} left to place — top or bottom.`
    : "Every card placed. Confirm to put them back.";

  const pool = $("arrange-pool");
  pool.innerHTML = "";
  for (const id of remaining) {
    const holder = document.createElement("div");
    holder.className = "choose-option";
    holder.appendChild(describe(id));

    const buttons = document.createElement("div");
    buttons.className = "arrange-buttons";
    for (const [label, pile] of [
      ["↑ Top", "top"],
      ["↓ Bottom", "bottom"],
    ]) {
      const b = document.createElement("button");
      b.className = "opt";
      b.textContent = label;
      b.addEventListener("click", () => {
        arrangePlacements.push({ id, pile });
        renderArrange(snap);
      });
      buttons.appendChild(b);
    }
    holder.appendChild(buttons);
    pool.appendChild(holder);
  }

  for (const [slotId, pile] of [
    ["arrange-top", arrangeTop],
    ["arrange-bottom", arrangeBottom],
  ]) {
    const slot = $(slotId);
    slot.innerHTML = "";
    if (pile.length === 0) {
      slot.innerHTML = `<div class="arrange-empty">nothing yet</div>`;
      continue;
    }
    pile.forEach((id, i) => {
      const holder = document.createElement("div");
      holder.className = "choose-option";
      holder.appendChild(describe(id));
      holder.appendChild(
        Object.assign(document.createElement("div"), {
          className: "choose-rank",
          textContent: String(i + 1),
        }),
      );
      slot.appendChild(holder);
    });
  }

  $("arrange-undo").disabled = placed.length === 0;
  $("arrange-confirm").disabled = remaining.length > 0;
  modal.hidden = animating();
}


$("arrange-undo").addEventListener("click", () => {
  arrangePlacements.pop();
  renderArrange(lastSnapshot);
});

$("arrange-confirm").addEventListener("click", () => {
  submitArrangement(lastSnapshot);
});

/** Submits the built arrangement by finding the option that is exactly it. */
function submitArrangement(snap) {
  const top = arrangePile("top");
  const wanted = [...top, ...arrangePile("bottom")];
  const opt = snap.options.find(
    (o) =>
      o.split === top.length &&
      o.cards.length === wanted.length &&
      o.cards.every((id, i) => id === wanted[i]),
  );
  if (!opt) {
    $("question").textContent = "That arrangement is not on offer";
    return;
  }
  arrangePlacements = [];
  $("arrange-modal").hidden = true;
  choose(opt.index);
}

/** Submits a set of cards by finding the option that names exactly them. */
function submitChoice(cards, snap) {
  const candidates = snap.choose_candidates ?? [];
  const want = classKey(cards, candidates);
  const opt =
    snap.options.find((o) => sameSet(o.cards, cards)) ??
    snap.options.find(
      (o) => o.cards.length === cards.length && classKey(o.cards, candidates) === want,
    );
  if (!opt) {
    // Into the modal, not the board behind it: the overlay is opaque, so a
    // refusal written to #question is invisible and Confirm looks inert.
    $("choose-sub").textContent = "That combination is not on offer.";
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

/** Every card drawn with a menu and a pin on it: the board, plus your hand.
 *
 *  Deliberately not the trash, which `cardIndex` does include. A trashed card is
 *  drawn `plain`, so it carries no click — a pin that followed one there would
 *  hold the panel on a dead card with nothing left to click it off. */
function pinnableIndex(view) {
  const index = boardIndex(view);
  for (const c of view.you.hand) index.set(c.id, c);
  return index;
}

/** Every card the viewer can click, for re-finding one across a re-render. */
function cardIndex(view) {
  const index = pinnableIndex(view);
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

/** The card number, for a `title` wherever a name appears on its own.
 *
 * A name does not identify a card — the same printed name recurs across sets
 * with different power, cost and text — so anywhere the number is not already
 * on screen, it belongs on hover.
 *
 * Empty for a card the viewer may not identify. The engine withholds the number
 * there, and an empty `title` shows no tooltip at all, where the string
 * "undefined" would read as the UI being broken rather than the information
 * being deliberately withheld.
 */
const cardNumber = (c) => (c && c.number) || "";

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
    <span class="atk" title="${cardNumber(attacker)}">${label(attacker)}</span>
    <span class="arrow">&#8594;</span>
    <span class="def" title="${cardNumber(defender)}">${label(defender)}</span>
    <span class="step">${view.battle.step}</span>
  `;

  // The full view. Power shown here is derived, so DON!! and counters are
  // already folded in — which is the whole point of showing it during the
  // Counter step.
  //
  // Held while something is flying: the K.O. that preceded this attack has to
  // be seen happening, or it reads as the attack's doing.
  modal.hidden = animating();
  $("battle-step").textContent = `${view.battle.step} step`;

  for (const [slot, card] of [["attacker", attacker], ["defender", defender]]) {
    const holder = $(`battle-${slot}`);
    holder.innerHTML = "";
    if (card) holder.appendChild(cardEl(card, { large: true, plain: true, preview: false }));
    $(`battle-${slot}-name`).textContent = cardName(card);
    $(`battle-${slot}-name`).title = cardNumber(card);
    $(`battle-${slot}-power`).textContent =
      card && card.power != null ? card.power : "—";
  }

  // 7-1-4-1: the attacker wins ties, so ">=" is the line that matters.
  //
  // Stated as a projection until the engine has actually resolved the battle.
  // The comparison is the reason to show this at all — it is what tells you
  // whether a Counter is worth spending — but before the Damage Step it is a
  // forecast of an unfinished battle, and calling it "attacker wins" while the
  // defender still holds decisions asserts an outcome nobody has reached.
  const ap = attacker && attacker.power;
  const dp = defender && defender.power;
  const resolved = beatsNow.some((b) => b.kind === "result");
  const verdict = $("battle-verdict");
  if (typeof ap === "number" && typeof dp === "number") {
    const wins = ap >= dp;
    verdict.textContent = resolved
      ? wins
        ? "the attack lands"
        : "the attack is repelled"
      : wins
        ? "as it stands, the attack lands"
        : "as it stands, it is repelled";
    verdict.className = `verdict ${wins ? "bad" : "good"}${resolved ? "" : " projected"}`;
  } else {
    verdict.textContent = "";
    verdict.className = "verdict";
  }
}

/** Beats from the snapshot currently being drawn. */
let beatsNow = [];

// ---- the result of a battle -------------------------------------------------
//
// Announced in its own overlay, not in the modal: the engine clears the battle
// as soon as it resolves, so the modal has already closed by the time there is
// a result to report.

const RESULT_HOLD = 2000;
let resultShown = null;
let resultTimer = null;

/** Announces how a battle ended, then fades. `text` is null between battles. */
function announceResult(text) {
  const box = $("battle-result");

  // The result stays in the snapshot until the next attack, so re-renders in
  // between must not restart the timer or it would never go away.
  if (text === resultShown) return;
  resultShown = text;
  clearTimeout(resultTimer);

  if (!text) {
    box.hidden = true;
    box.classList.remove("shown");
    return;
  }

  box.textContent = text;
  box.className = text.includes("repelled") ? "repelled" : "landed";
  box.hidden = false;
  // Next frame, so the transition has an initial state to move from.
  requestAnimationFrame(() => box.classList.add("shown"));

  resultTimer = setTimeout(() => {
    box.classList.remove("shown");
    // Hidden only once the fade has finished, so it cannot be tabbed to or
    // caught mid-transition by the next render.
    resultTimer = setTimeout(() => {
      box.hidden = true;
    }, 200);
  }, RESULT_HOLD);
}

/** What the defender has done so far, as narration with the cards involved.
 *
 * The board shows the *result* of a block or a Counter — the target changes,
 * the power goes up — but never says who did it or with what, and by the time
 * the modal closes the cards are back in the trash. */
function renderBeats(snap) {
  const box = $("battle-beats");
  box.innerHTML = "";
  const index = cardIndex(snap.view);

  for (const b of snap.battle_beats) {
    const row = document.createElement("div");
    row.className = `beat ${b.kind}`;

    const card = b.card == null ? null : index.get(b.card);
    if (card) {
      row.appendChild(cardEl(card, { small: true, plain: true, preview: false }));
    } else {
      row.appendChild(
        Object.assign(document.createElement("div"), { className: "beat-nocard" }),
      );
    }
    row.appendChild(
      Object.assign(document.createElement("div"), {
        className: "beat-text",
        textContent: b.text,
      }),
    );
    box.appendChild(row);
  }
  box.scrollTop = box.scrollHeight;
}

// ---- a card going to the trash ----------------------------------------------
//
// The pile just gains one, with nothing to say a card moved. Its new top card
// grows into place, the same way an arrival in hand does, and the reason it is
// there — "K.O.'d by Killer" — becomes the pile's tooltip. Nothing carries the
// reason across the board any more, so this is the only place it can live.

/** How long a card takes to grow into place, from the CSS. */
const GROW_MS = 450;

let trashSeenFor = null;
let growUntil = 0;

const animating = () => Date.now() < growUntil;

function noteTrashArrivals(snap) {
  // Guarded by snapshot identity: one snapshot can be rendered more than once
  // and must not restart the hold each time.
  if (snap === trashSeenFor) return;
  trashSeenFor = snap;

  for (const entry of snap.to_trash) {
    const info = catalogue.get(entry.number);
    lastCause.set(
      entry.yours ? "you" : "opp",
      `${info ? info.name : entry.number} — ${entry.cause}`,
    );
  }
  if (!snap.to_trash.length) return;

  // Modals wait for it, so cause is seen before effect: a card K.O.'d by an
  // [On Play] before an attack must be watched landing, not discovered once
  // the battle it had nothing to do with has resolved.
  growUntil = Date.now() + GROW_MS;
  setTimeout(() => lastSnapshot && render(lastSnapshot), GROW_MS + 30);
}

// ---- where in the turn we are -----------------------------------------------
//
// The top bar named the current phase, which is only useful to a player who
// already knows what the phases are and what order they come in. Showing the
// whole sequence with the current one lit says the same thing to someone who
// does not.

/** The turn's phases in order (6-1), keyed by what `Phase` serialises to. */
const TURN_PHASES = [
  ["Refresh", "Refresh"],
  ["Draw", "Draw"],
  // The only one whose printed name is not its variant name.
  ["Don", "DON!!"],
  ["Main", "Main"],
  ["End", "End"],
];

function renderPhases(phase) {
  const box = $("phase-steps");
  const at = TURN_PHASES.findIndex(([key]) => key === phase);
  box.innerHTML = "";
  TURN_PHASES.forEach(([, label], i) => {
    const step = document.createElement("span");
    // An unrecognised phase leaves `at` at -1, which lights nothing rather
    // than lighting the first one and lying about where the turn is.
    step.className = "step" + (i === at ? " now" : i < at ? " done" : "");
    step.textContent = label;
    box.appendChild(step);
  });
}

// ---- whose turn it is -------------------------------------------------------
//
// The turn label in the top bar changes without anyone looking at it. A turn
// passing is the largest thing that happens in the game and deserves to be
// unmissable once, rather than permanently.

const TURN_HOLD = 1500;
let lastTurn = null;
let turnTimer = null;

function announceTurn(snap) {
  const turn = snap.view.turn;
  // Turn 0 is setup, which nobody owns.
  if (turn < 1 || turn === lastTurn) return;
  lastTurn = turn;

  const box = $("turn-banner");
  clearTimeout(turnTimer);
  box.className = snap.turn_label.startsWith("Your") ? "yours" : "theirs";
  box.innerHTML = `
    <div class="turn-who">${snap.turn_label}</div>
    <div class="turn-no">Turn ${turn}</div>
  `;
  box.hidden = false;
  requestAnimationFrame(() => box.classList.add("shown"));

  turnTimer = setTimeout(() => {
    box.classList.remove("shown");
    turnTimer = setTimeout(() => {
      box.hidden = true;
    }, 260);
  }, TURN_HOLD);
}

// ---- the Life card that came off ---------------------------------------------
//
// Taking damage offers "Activate [Trigger]" or "Take into hand" — a choice
// about a card the player has not seen. The card is theirs to see either way:
// declining adds it to their hand unrevealed (10-1-5-2).

async function renderTriggerTray(snap) {
  const tray = $("trigger-tray");
  const number = snap.trigger_card;
  if (!number) {
    tray.hidden = true;
    return;
  }

  const info = catalogue.get(number);
  const uri = await art(number);
  if (snap !== lastSnapshot) return; // superseded while the art resolved

  tray.innerHTML = `
    <div class="trigger-head">This Life card has a [Trigger]</div>
    <div class="trigger-body">
      <div class="trigger-art">${uri ? `<img src="${uri}" alt="${number}" />` : number}</div>
      <div class="trigger-text">
        <div class="pname">${info ? info.name : number}</div>
        <div class="pmeta">${
          info
            ? `${info.category} · cost ${info.cost}${info.power != null ? ` · ${info.power}` : ""}`
            : number
        }</div>
        ${info && info.trigger ? `<div class="ptrigger">${info.trigger}</div>` : ""}
        ${info && info.effect ? `<div class="peffect">${info.effect.replaceAll("<br>", "<br/>")}</div>` : ""}
      </div>
    </div>
  `;
  tray.hidden = false;
}

// ---- the Counter step -------------------------------------------------------
//
// The flat list named the cards involved and nothing more, which does not
// identify *which* Zoro when two are in play, and the modal blurs the board it
// is talking about. The cards are shown here instead, in the modal that is
// asking about them.
//
// Only counters aimed at the card under attack are shown. Boosting a different
// battler is legal and occasionally right, so those stay in the list below —
// they are rare enough not to be worth a second selection step for.

/** The Counter value a hand card is worth, from its printed details. */
function counterValue(card) {
  const info = card && card.number ? catalogue.get(card.number) : null;
  return info && info.counter != null ? info.counter : null;
}

function renderCounterTray(snap) {
  const tray = $("counter-tray");
  const view = snap.view;
  if (snap.pending_kind !== "counter" || !view.battle) {
    tray.hidden = true;
    return;
  }

  const defender = view.battle.target;
  const index = cardIndex(view);
  const target = index.get(defender);
  const basePower = target && target.power != null ? target.power : null;

  const options = snap.options.filter(
    (o) => o.kind === "counter" && o.cards[1] === defender,
  );

  $("counter-prompt").textContent = options.length
    ? `Play a Counter for ${cardName(target)}`
    : `No Counter in hand for ${cardName(target)}`;
  $("counter-prompt").title = cardNumber(target);

  const hand = $("counter-hand");
  hand.innerHTML = "";

  const power = $("battle-defender-power");
  const restore = () => {
    power.textContent = basePower == null ? "—" : basePower;
    power.classList.remove("boosted");
  };

  for (const opt of options) {
    const card = index.get(opt.cards[0]);
    if (!card) continue;

    const holder = document.createElement("div");
    holder.className = "counter-option";
    holder.appendChild(cardEl(card, { small: true, plain: true, preview: false }));

    // An Event played as a Counter has no printed Counter value; its worth is
    // whatever its text does, so it is labelled rather than given a number.
    const value = counterValue(card);
    holder.appendChild(
      Object.assign(document.createElement("div"), {
        className: "counter-value",
        textContent: value == null ? "[Counter]" : `+${value}`,
      }),
    );

    holder.addEventListener("mouseenter", () => {
      if (basePower == null || value == null) return;
      power.textContent = basePower + value;
      power.classList.add("boosted");
    });
    holder.addEventListener("mouseleave", restore);
    holder.addEventListener("click", () => {
      restore();
      choose(opt.index);
    });
    hand.appendChild(holder);
  }

  tray.hidden = false;
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
  beatsNow = snap.battle_beats ?? [];

  // An option index only means anything against the decision it was read from,
  // so neither a target picker nor a staged activation can survive a *new*
  // snapshot. Their cards are a copy taken when they opened, too, and would go
  // on showing a board that has moved on.
  //
  // Against a new one, not against a re-render: one snapshot is drawn more than
  // once — the trash animation redraws the same one 480ms later — and closing
  // on that would take a picker out from under the cursor mid-decision. Same
  // reasoning as `chooseDismissedFor`.
  if (openedFor !== snap) {
    closeAttackPicker();
    cancelActivation();
  }

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
  renderPhases(view.phase);
  $("phase-label").textContent = `turn ${view.turn}`;

  // Everything else these used to spell out — life, deck, DON!! — is on the
  // board now as the pile or the row itself. A hidden hand is the exception.
  $("opp-hand").textContent = `opponent hand ${view.opponent.hand_count}`;

  renderSide(view, view.opponent, "opp");
  renderSide(view, view.you, "you");
  renderLife(view.opponent, "opp");
  renderLife(view.you, "you");
  renderDeck(view.opponent, "opp");
  renderDeck(view.you, "you");
  renderTrash(view.opponent, "opp", "Opponent");
  renderTrash(view.you, "you", "Your");
  renderDon(view.opponent, "opp", view.opponent.don_deck);
  renderDon(view.you, "you", view.you.don_deck);

  const hand = $("hand");
  hand.innerHTML = "";
  if (view.you.hand.length === 0) {
    hand.innerHTML = `<div class="empty">hand empty</div>`;
  }
  // Diffed by instance id rather than taken from the snapshot: several copies
  // of a card are indistinguishable by number, and the id says which is new.
  const byId = new Map(view.you.hand.map((c) => [c.id, c]));
  handOrder = handOrder.filter((id) => byId.has(id));
  for (const c of view.you.hand) {
    if (!handOrder.includes(c.id)) handOrder.push(c.id);
  }

  let nth = 0;
  for (const id of handOrder) {
    const isNew = !lastHandIds.has(id);
    hand.appendChild(cardSlot(byId.get(id), { yours: true, arriving: isNew ? ++nth : 0 }));
  }
  lastHandIds = new Set(handOrder);

  // Before the modals: both gate themselves on `animating()`, and this is what
  // opens that window. Called after them, it sets `growUntil` too late to be
  // read this pass, so a snapshot carrying both a K.O. and a battle decision
  // shows the modal over the very animation it is meant to wait for.
  noteTrashArrivals(snap);

  renderBattle(view);
  if (view.battle) renderBeats(snap);
  renderCounterTray(snap);
  renderTriggerTray(snap);
  announceTurn(snap);
  announceResult(snap.battle_result ?? null);
  renderChoose(snap);
  renderArrange(snap);

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

  // A Choose that was waved away is still owed, so the way back to it sits at
  // the top of the list it was collapsing.
  if (chooseDismissedFor === snap && snap.choose_up_to != null) {
    const again = document.createElement("button");
    again.className = "opt";
    again.textContent = "Show the choices again";
    again.addEventListener("click", () => {
      chooseDismissedFor = null;
      render(snap);
    });
    $(inBattle ? "battle-options" : "options").prepend(again);
  }

  // The board was rebuilt underneath both panels. The menu simply closes: its
  // card element is gone, so no mouseleave can ever arrive to close it later,
  // and hovering again costs nothing.
  //
  // The pending open goes unconditionally, ahead of that test: a hover less than
  // OPEN_DELAY old has not set `selected` yet, so `closeMenu` would not be
  // reached to clear it — and the timer would then fire against a detached
  // element, parking an unclosable menu in the corner off a zero-sized measure.
  clearTimeout(openTimer);
  if (selected !== null) closeMenu();

  // A pinned preview outlives the render, and is redrawn rather than left as it
  // was — the card's power is exactly the kind of thing that just changed. It
  // is released if that card has left play, or if a battle has taken the board.
  if (pinnedId !== null) {
    const card = pinnableIndex(view).get(pinnedId);
    if (card && !inBattle) showPreview(card);
    else unpinPreview();
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

// An index only means anything against the decision that was pending when it
// was read. Clearing `#options` is not enough to enforce that: a battle puts
// the buttons in `#battle-options` and the counters in their own tray, and
// both stay on screen until the next snapshot renders. A second click landing
// before then would be validated against the *next* pending decision, which
// silently applies a legal action nobody picked — declining a block twice
// plays whichever Counter now occupies that slot. Guard the submission itself
// rather than the surfaces, so a surface added later is covered too.
let inFlight = false;

async function choose(index) {
  if (inFlight) return;
  inFlight = true;
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
  } finally {
    inFlight = false;
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
    // A new game starts at turn 1 again, which is a change worth announcing.
    lastTurn = null;
    // Cleared so the first render of a fresh Life area is not five losses.
    lastLifeCount.clear();
    lastTrashTop.clear();
    lastCause.clear();
    // Seeded from the opening hand so a deal does not read as five arrivals.
    handOrder = result.snapshot.view.you.hand.map((c) => c.id);
    lastHandIds = new Set(handOrder);
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

/** Fills both deck pickers from the backend's list.
 *
 *  The menu is generated rather than written into `index.html` so that a newly
 *  scripted set appears by being added to `op_cards::decks::ALL` alone. The two
 *  lists default to different decks, which is only a nicety — nothing stops a
 *  mirror match. */
async function loadDecks() {
  const decks = await invoke("decks");
  if (!decks.length) return;
  for (const [id, fallback] of [
    ["your-deck", 0],
    ["ai-deck", Math.min(1, decks.length - 1)],
  ]) {
    const select = $(id);
    select.innerHTML = "";
    for (const deck of decks) {
      const opt = document.createElement("option");
      opt.value = deck.id;
      opt.textContent = deck.name;
      select.appendChild(opt);
    }
    select.selectedIndex = fallback;
  }
}

loadDecks();
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
