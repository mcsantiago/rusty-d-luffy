#!/usr/bin/env python3
"""Reports Rust comment blocks longer than AGENTS.md allows.

    python3 tools/lint/comments.py            # report, exit 0
    python3 tools/lint/comments.py --strict   # exit 1 if anything is over
    python3 tools/lint/comments.py --stats    # counts by kind, no listing

A block is a run of consecutive comment lines; fenced code in a doc comment is
not counted, since an example is not prose. Budgets and the reasoning behind
them live in AGENTS.md, under "Comments".
"""

import argparse
import pathlib
import sys

# Kind -> (prefix, ceiling). See AGENTS.md, "Comments".
BUDGETS = {
    "module": 10,
    "item": 4,
    "inline": 4,
}

# Card scripts keep each card's printed text above it, which is longer than any
# budget and is the point of them (AGENTS.md, "Adding a card set").
EXEMPT = ("crates/op-cards/src/sets/",)


def kind_of(line):
    if line.startswith("//!"):
        return "module"
    if line.startswith("///"):
        return "item"
    return "inline"


def blocks(path):
    """Every comment block in `path`, as (line number, kind, prose length)."""
    out, run, start = [], [], 0
    fenced = False
    for number, raw in enumerate(path.read_text().splitlines(), 1):
        line = raw.strip()
        if line.startswith(("///", "//!", "//")):
            if not run:
                start, fenced = number, False
            body = line.lstrip("/!").strip()
            if body.startswith("```"):
                fenced = not fenced
                continue
            if not fenced:
                run.append(line)
        else:
            if run:
                out.append((start, kind_of(run[0]), len(run)))
            run = []
    if run:
        out.append((start, kind_of(run[0]), len(run)))
    return out


def main():
    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("paths", nargs="*", default=["crates"], help="files or dirs (default: crates)")
    parser.add_argument("--strict", action="store_true", help="exit 1 on any violation")
    parser.add_argument("--stats", action="store_true", help="counts by kind, no listing")
    args = parser.parse_args()

    files = []
    for root in args.paths:
        root = pathlib.Path(root)
        files.extend([root] if root.is_file() else sorted(root.rglob("*.rs")))

    over, counted = [], 0
    for path in files:
        if any(str(path).startswith(prefix) for prefix in EXEMPT):
            continue
        for line, kind, length in blocks(path):
            counted += 1
            if length > BUDGETS[kind]:
                over.append((length - BUDGETS[kind], length, kind, path, line))

    if args.stats:
        for kind, ceiling in BUDGETS.items():
            hits = [o for o in over if o[2] == kind]
            excess = sum(o[0] for o in hits)
            print(f"  {kind:7} ceiling {ceiling:2}  {len(hits):4} over, {excess} lines to cut")
    else:
        # Furthest over budget first, which is where the most cutting is owed.
        for _, length, kind, path, line in sorted(over, reverse=True):
            print(f"{path}:{line}: {kind} comment is {length} lines, ceiling {BUDGETS[kind]}")

    print(
        f"\n{len(over)} of {counted} comment blocks over budget"
        f" ({sum(o[0] for o in over)} lines to cut)"
    )
    return 1 if (over and args.strict) else 0


if __name__ == "__main__":
    sys.exit(main())
