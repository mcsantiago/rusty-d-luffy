#!/usr/bin/env python3
"""Fetch One Piece Card Game card data into data/.

Source: https://github.com/buhbbl/punk-records — static versioned JSON generated
by vegapull (https://github.com/Coko7/vegapull) from the official Bandai site.

Card text and images are Bandai's copyright. Everything this writes lands in
data/, which is gitignored; nothing here is vendored into the repo.

Usage:
    python3 tools/ingest/fetch_cards.py                 # ST-01 and ST-02 only
    python3 tools/ingest/fetch_cards.py --packs ST-03 OP-01
    python3 tools/ingest/fetch_cards.py --packs PROMO
    python3 tools/ingest/fetch_cards.py --all

Alternate printings (OP01-016_p1, EB01-006_r1) are skipped: they are the same
card as their base, including for the four-copy deck limit.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

RAW = "https://raw.githubusercontent.com/buhbbl/punk-records/main/english"
API = "https://api.github.com/repos/buhbbl/punk-records/contents/english"

REPO_ROOT = Path(__file__).resolve().parents[2]
DATA = REPO_ROOT / "data"

DEFAULT_PACKS = ["ST-01", "ST-02"]


def get(url: str) -> bytes:
    req = urllib.request.Request(url, headers={"User-Agent": "OnePieceSim-ingest"})
    with urllib.request.urlopen(req, timeout=60) as resp:
        return resp.read()


def get_json(url: str):
    return json.loads(get(url))


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
    packs = json.loads(raw)

    out = {}
    for pack_id, meta in packs.items():
        for alias in pack_aliases(meta):
            out.setdefault(alias, []).append(pack_id)
    return out


def is_art_variant(filename: str) -> bool:
    """Alternate printings — `OP01-016_p1.json`, `EB01-006_r1.json`.

    These are the same card as their base: same characteristics, and same card
    number for the four-copy deck limit (5-1-2-3). The engine drops them at load
    (`op_core::card::is_art_variant`); skipping the download too avoids fetching
    ~2,000 files nothing reads.
    """
    return "_" in filename


def fetch_pack(label: str, pack_id: str) -> int:
    listing = get_json(f"{API}/cards/{pack_id}")
    names = [
        e["name"]
        for e in listing
        if e["type"] == "file" and not is_art_variant(e["name"])
    ]
    dest = DATA / "cards" / pack_id
    dest.mkdir(parents=True, exist_ok=True)

    def one(name: str) -> bool:
        try:
            (dest / name).write_bytes(get(f"{RAW}/cards/{pack_id}/{name}"))
            return True
        except urllib.error.HTTPError as exc:
            print(f"  ! {name}: HTTP {exc.code}", file=sys.stderr)
            return False

    with ThreadPoolExecutor(max_workers=8) as pool:
        ok = sum(pool.map(one, names))
    print(f"  {label} ({pack_id}): {ok}/{len(names)} cards")
    return ok


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--packs", nargs="+", metavar="NAME",
                    help="packs to fetch, e.g. ST-01 OP-01 PROMO (case and "
                         "dashes are ignored)")
    ap.add_argument("--all", action="store_true", help="fetch every pack")
    args = ap.parse_args()

    print("fetching pack index...")
    packs = load_packs()

    if args.all:
        wanted = sorted(packs)
    else:
        wanted = args.packs or DEFAULT_PACKS

    unknown = [p for p in wanted if normalize(p) not in packs]
    if unknown:
        print(f"unknown pack name(s): {', '.join(unknown)}", file=sys.stderr)
        print(f"known: {', '.join(sorted(packs))}", file=sys.stderr)
        return 1

    # A name may resolve to several products — EB-04 shipped inside both the
    # OP-14 and OP-15 boosters — so fetch every pack it names, without
    # re-fetching a product two names share.
    selected = {}
    for name in wanted:
        for pack_id in packs[normalize(name)]:
            selected.setdefault(pack_id, name)

    print(f"fetching {len(selected)} product(s) into {DATA}")
    total = 0
    for pack_id, name in selected.items():
        total += fetch_pack(name, pack_id)

    # Manifest maps the names asked for onto the numeric pack dirs.
    manifest = {
        "packs": {name: packs[normalize(name)] for name in wanted},
        "cards": total,
    }
    (DATA / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"done: {total} cards")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
