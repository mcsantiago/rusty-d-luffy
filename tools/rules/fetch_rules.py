#!/usr/bin/env python3
"""Fetch the official Comprehensive Rules into data/rules/.

Source: https://www.optcg.one/rules/one-piece-tcg-comprehensive-rules.pdf

The Comprehensive Rules are Bandai's copyright. Like the card data, everything
this writes lands in data/, which is gitignored; nothing here is vendored into
the repo.

The PDF is the citable artefact but a poor thing to read programmatically, so
this also writes a plain-text rendering next to it. That text is what the
`rules-auditor` agent loads: at ~10,600 words the whole document fits in a
context window, and clause numbers survive extraction in the same `7-1-4-1.`
form the code cites, so grepping for a clause finds it.

Usage:
    python3 tools/rules/fetch_rules.py
    python3 tools/rules/fetch_rules.py --refresh   # re-download

Requires pypdf (`pip install pypdf`) for the text rendering. Without it the PDF
is still fetched and the extraction is skipped with a warning.
"""

from __future__ import annotations

import argparse
import re
import sys
import urllib.error
import urllib.request
from pathlib import Path

URL = "https://www.optcg.one/rules/one-piece-tcg-comprehensive-rules.pdf"

REPO = Path(__file__).resolve().parents[2]
OUT_DIR = REPO / "data" / "rules"
PDF = OUT_DIR / "comprehensive-rules.pdf"
TXT = OUT_DIR / "comprehensive-rules.txt"

# Where the codebase records which version it was written against. Kept as the
# single source of truth so this script cannot disagree with the citations.
CITED_BY = REPO / "crates" / "op-core" / "src" / "lib.rs"


def cited_version() -> str | None:
    """The rules version op-core's citations claim to follow.

    The version sits in a doc comment and wraps across lines, so the `//!`
    prefixes have to come out before matching — otherwise this returns None on
    a perfectly good header and the drift warning below silently never fires.
    """
    try:
        text = CITED_BY.read_text(encoding="utf-8")
    except OSError:
        return None
    text = re.sub(r"^\s*//[/!]?", " ", text, flags=re.MULTILINE)
    found = re.search(r"Comprehensive\s+Rules\W*\s*v(\d+\.\d+\.\d+)", text)
    return found.group(1) if found else None


def published_version(text: str) -> str | None:
    found = re.search(r"Version\s+(\d+\.\d+\.\d+)", text)
    return found.group(1) if found else None


def fetch(refresh: bool) -> bool:
    if PDF.exists() and not refresh:
        print(f"{PDF.relative_to(REPO)} already present; --refresh to re-download")
        return True

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    print(f"fetching {URL}")
    request = urllib.request.Request(URL, headers={"User-Agent": "OnePieceSim/rules-fetch"})
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            body = response.read()
    except (urllib.error.URLError, TimeoutError) as err:
        print(f"could not fetch the rules: {err}", file=sys.stderr)
        return False

    if not body.startswith(b"%PDF"):
        print(f"{URL} did not return a PDF", file=sys.stderr)
        return False

    PDF.write_bytes(body)
    print(f"wrote {PDF.relative_to(REPO)} ({len(body):,} bytes)")
    return True


def extract() -> str | None:
    try:
        import pypdf
    except ImportError:
        print(
            "pypdf is not installed, so only the PDF was fetched.\n"
            "  pip install pypdf   # then re-run to write the text rendering",
            file=sys.stderr,
        )
        return None

    reader = pypdf.PdfReader(PDF)
    # Page markers are worth keeping: they are how a finding cites a location
    # that has no clause number of its own, such as a table.
    pages = [
        f"--- page {n} ---\n{page.extract_text()}"
        for n, page in enumerate(reader.pages, start=1)
    ]
    text = "\n".join(pages)
    TXT.write_text(text, encoding="utf-8")
    print(f"wrote {TXT.relative_to(REPO)} ({len(reader.pages)} pages, {len(text.split()):,} words)")
    return text


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--refresh", action="store_true", help="re-download even if present")
    args = parser.parse_args()

    if not fetch(args.refresh):
        return 1

    text = extract()
    if text is None:
        return 1

    published = published_version(text)
    cited = cited_version()
    print(f"published version: {published or 'unknown'}")
    if published and cited and published != cited:
        # Not an error: the upstream document moving is normal. It does mean
        # every "8-4-1-1"-style citation in the tree was written against a
        # different edition and is now worth re-checking.
        print(
            f"\nWARNING: op-core cites v{cited}, upstream is now v{published}.\n"
            "Clause numbers move between editions; re-check the citations before\n"
            "trusting them, and update the header in crates/op-core/src/lib.rs.",
            file=sys.stderr,
        )

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
