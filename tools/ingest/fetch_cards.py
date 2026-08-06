#!/usr/bin/env python3
"""Fetch One Piece Card Game card data into data/.

Source: https://github.com/buhbbl/punk-records — static versioned JSON generated
by vegapull (https://github.com/Coko7/vegapull) from the official Bandai site.

Card text and images are Bandai's copyright. Everything this writes lands in
data/, which is gitignored; nothing here is vendored into the repo.

Usage:
    python3 tools/ingest/fetch_cards.py                 # ST-01 and ST-02 only
    python3 tools/ingest/fetch_cards.py --packs ST-03 OP-01
    python3 tools/ingest/fetch_cards.py --all
"""

from __future__ import annotations

import argparse
import json
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


def load_packs() -> dict:
    """pack label (e.g. "ST-01") -> pack id (e.g. "569001")."""
    DATA.mkdir(parents=True, exist_ok=True)
    raw = get(f"{RAW}/packs.json")
    (DATA / "packs.json").write_bytes(raw)
    packs = json.loads(raw)
    out = {}
    for pack_id, meta in packs.items():
        label = meta.get("title_parts", {}).get("label")
        if label:
            out[label] = pack_id
    return out


def fetch_pack(label: str, pack_id: str) -> int:
    listing = get_json(f"{API}/cards/{pack_id}")
    names = [e["name"] for e in listing if e["type"] == "file"]
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
    ap.add_argument("--packs", nargs="+", metavar="LABEL",
                    help="pack labels to fetch, e.g. ST-01 OP-01")
    ap.add_argument("--all", action="store_true", help="fetch every pack")
    args = ap.parse_args()

    print("fetching pack index...")
    packs = load_packs()

    if args.all:
        wanted = sorted(packs)
    else:
        wanted = args.packs or DEFAULT_PACKS

    unknown = [p for p in wanted if p not in packs]
    if unknown:
        print(f"unknown pack label(s): {', '.join(unknown)}", file=sys.stderr)
        print(f"known: {', '.join(sorted(packs))}", file=sys.stderr)
        return 1

    print(f"fetching {len(wanted)} pack(s) into {DATA}")
    total = 0
    for label in wanted:
        total += fetch_pack(label, packs[label])

    # Manifest maps labels the engine understands onto the numeric pack dirs.
    manifest = {"packs": {label: packs[label] for label in wanted}, "cards": total}
    (DATA / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"done: {total} cards")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
