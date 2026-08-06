#!/usr/bin/env python3
"""Fetch One Piece Card Game card data into data/.

Source: https://github.com/buhbbl/punk-records — static versioned JSON generated
by vegapull (https://github.com/Coko7/vegapull) from the official Bandai site.

Card text and images are Bandai's copyright. Everything this writes lands in
data/, which is gitignored; nothing here is vendored into the repo.

One request per product: upstream publishes a per-pack aggregate under
`english/data/<pack_id>.json` carrying the same fields as the individual card
files, so fetching every set is 59 requests rather than ~2,700. Nothing here
touches the GitHub API, so the 60-requests-per-hour unauthenticated limit does
not apply.

Usage:
    python3 tools/ingest/fetch_cards.py                 # ST-01 and ST-02 only
    python3 tools/ingest/fetch_cards.py --packs ST-03 OP-01
    python3 tools/ingest/fetch_cards.py --packs PROMO
    python3 tools/ingest/fetch_cards.py --all
    python3 tools/ingest/fetch_cards.py --all --refresh # re-download everything

Already-fetched packs are skipped, so an interrupted run resumes where it left
off. Alternate printings (OP01-016_p1, EB01-006_r1) are dropped: they are the
same card as their base, including for the four-copy deck limit.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import time
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

RAW = "https://raw.githubusercontent.com/buhbbl/punk-records/main/english"

REPO_ROOT = Path(__file__).resolve().parents[2]
DATA = REPO_ROOT / "data"
CARDS = DATA / "cards"

DEFAULT_PACKS = ["ST-01", "ST-02"]

RETRIES = 4
BACKOFF = 2.0


def get(url: str, timeout: int = 30) -> bytes:
    """Fetches a URL, retrying transient failures with exponential backoff.

    Connection timeouts are the common failure when pulling many files in a row
    — GitHub stalls the TLS handshake rather than refusing it — so `URLError`
    has to be retried, not just bad HTTP statuses. A retry loop here is what
    keeps one blip from discarding a whole run's progress.
    """
    last = None
    for attempt in range(RETRIES):
        try:
            req = urllib.request.Request(
                url, headers={"User-Agent": "OnePieceSim-ingest"}
            )
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                return resp.read()
        except urllib.error.HTTPError as exc:
            # 404 is a real answer; retrying will not change it.
            if exc.code == 404:
                raise
            last = exc
        except (urllib.error.URLError, TimeoutError, OSError) as exc:
            last = exc

        if attempt < RETRIES - 1:
            delay = BACKOFF**attempt
            print(f"    retrying in {delay:.0f}s ({last})", file=sys.stderr)
            time.sleep(delay)
    raise RuntimeError(f"giving up on {url} after {RETRIES} attempts: {last}")


def normalize(name: str) -> str:
    """"ST-01", "st01", "ST 01" all key the same pack."""
    return re.sub(r"[^A-Z0-9]", "", name.upper())


# Packs upstream leaves unlabelled, keyed by their raw title. Promos are
# tournament-legal, so they need to be fetchable by name.
UNLABELLED = {
    "Promotion card": ["PROMO", "P"],
    "Other Product Card": ["OTHER"],
}


def pack_aliases(meta: dict) -> list:
    """Every name a pack should answer to.

    Upstream labels are not always a single product code: OP-14 and OP-15 ship
    as combined products labelled "OP14-EB04" and "OP15-EB04". Each component is
    registered separately so `--packs OP-14` resolves, and a component shared by
    two products (EB04) resolves to both.
    """
    parts = meta.get("title_parts", {})
    label = parts.get("label")
    if not label:
        return [normalize(a) for a in UNLABELLED.get(meta.get("raw_title", ""), [])]

    names = {normalize(label)}
    for component in label.split("-"):
        # A product code is letters followed by digits, e.g. "OP14", "EB04".
        # This skips the bare "ST"/"01" halves of an ordinary "ST-01" label,
        # which the whole-label entry already covers.
        if re.fullmatch(r"[A-Z]+\d+", component):
            names.add(normalize(component))
    return sorted(names)


def load_packs() -> dict:
    """alias (e.g. "ST01") -> list of pack ids (e.g. ["569001"])."""
    DATA.mkdir(parents=True, exist_ok=True)
    raw = get(f"{RAW}/packs.json")
    (DATA / "packs.json").write_bytes(raw)

    out = {}
    for pack_id, meta in json.loads(raw).items():
        for alias in pack_aliases(meta):
            out.setdefault(alias, []).append(pack_id)
    return out


def is_art_variant(card_id: str) -> bool:
    """Alternate printings — `OP01-016_p1`, `EB01-006_r1`.

    These are the same card as their base: same characteristics, and the same
    card number that the four-copy deck limit counts against (5-1-2-3). The
    engine drops them at load too (`op_core::card::is_art_variant`), so a stale
    data/ cannot reintroduce them.
    """
    return "_" in card_id


def fetch_pack(name: str, pack_id: str, refresh: bool) -> int:
    dest = CARDS / f"{pack_id}.json"
    if dest.exists() and not refresh:
        cards = json.loads(dest.read_text())
        print(f"  {name:12} ({pack_id})  {len(cards):>4} cards  (cached)")
        return len(cards)

    cards = json.loads(get(f"{RAW}/data/{pack_id}.json"))
    kept = [c for c in cards if not is_art_variant(c.get("id", ""))]

    dest.write_text(json.dumps(kept, indent=1) + "\n")

    skipped = len(cards) - len(kept)
    note = f"  ({skipped} alternate printings skipped)" if skipped else ""
    print(f"  {name:12} ({pack_id})  {len(kept):>4} cards{note}")
    return len(kept)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--packs", nargs="+", metavar="NAME",
                    help="packs to fetch, e.g. ST-01 OP-01 PROMO (case and "
                         "dashes are ignored)")
    ap.add_argument("--all", action="store_true", help="fetch every pack")
    ap.add_argument("--refresh", action="store_true",
                    help="re-download packs already present")
    ap.add_argument("--list", action="store_true",
                    help="list available pack names and exit")
    ap.add_argument("--jobs", type=int, default=4, metavar="N",
                    help="parallel downloads (default 4; use 1 if throttled)")
    args = ap.parse_args()

    print("fetching pack index...")
    packs = load_packs()

    if args.list:
        print(f"{len(packs)} names available:\n  {', '.join(sorted(packs))}")
        return 0

    wanted = sorted(packs) if args.all else (args.packs or DEFAULT_PACKS)

    unknown = [p for p in wanted if normalize(p) not in packs]
    if unknown:
        print(f"unknown pack name(s): {', '.join(unknown)}", file=sys.stderr)
        print("run with --list to see the available names", file=sys.stderr)
        return 1

    # A name may resolve to several products — EB-04 shipped inside both the
    # OP-14 and OP-15 boosters — so fetch every pack it names, without
    # re-fetching a product two names share.
    selected = {}
    for name in wanted:
        for pack_id in packs[normalize(name)]:
            selected.setdefault(pack_id, name)

    print(f"fetching {len(selected)} product(s) into {CARDS} with {args.jobs} job(s)")
    CARDS.mkdir(parents=True, exist_ok=True)

    # Kept deliberately low. An earlier version fetched one file per *card* —
    # ~2,700 requests — across 8 workers, and GitHub responded by stalling the
    # TLS handshake until the run died. Per-pack aggregates cut that to 59
    # requests, which is comfortably under any throttling threshold; a handful
    # of workers shaves the wall time without going back towards it. Use
    # `--jobs 1` if a network ever objects.
    total = 0
    failed = []
    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        futures = {
            pool.submit(fetch_pack, name, pack_id, args.refresh): (name, pack_id)
            for pack_id, name in selected.items()
        }
        for future in futures:
            name, pack_id = futures[future]
            try:
                total += future.result()
            except Exception as exc:
                # One unreachable product should not discard the rest of the
                # run; re-running picks up only what is missing.
                print(f"  {name:12} ({pack_id})  FAILED: {exc}", file=sys.stderr)
                failed.append(name)

    manifest = {
        "packs": {name: packs[normalize(name)] for name in wanted},
        "cards": total,
    }
    (DATA / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")

    print(f"done: {total} cards across {len(selected) - len(failed)} product(s)")
    if failed:
        print(f"{len(failed)} failed: {', '.join(failed)} — re-run to retry",
              file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
