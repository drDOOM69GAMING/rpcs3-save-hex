#!/usr/bin/env python3
"""Regenerate `assets/titles.tsv` from the community PS3 game database.

GameDB-PS3 (https://github.com/niemasd/GameDB-PS3) publishes a TSV of
`ID, title, language, region, release_date, serial`. We normalise the serial
(strip the hyphen, upper-case it) and keep `SERIAL<TAB>title`, which is the
fallback list used to name saves whose game has no cheat codes.

Usage:
    python3 tools/fetch_titles.py [OUTPUT_TSV]

Defaults to `assets/titles.tsv` next to this script's parent directory.
"""
import csv
import io
import re
import sys
import urllib.request
from pathlib import Path

URL = "https://github.com/niemasd/GameDB-PS3/releases/latest/download/PS3.data.tsv"
DEFAULT_OUT = Path(__file__).resolve().parent.parent / "assets" / "titles.tsv"

BAD_URL = re.compile(r"https?://|www\.|\.com|\.net|\.org", re.I)
TRADEMARK = str.maketrans("", "", "\u2122\u00ae\u00a9")


def fetch() -> str:
    print(f"downloading {URL} ...")
    with urllib.request.urlopen(URL, timeout=120) as resp:
        return resp.read().decode("utf-8", "replace")


def parse(raw: str) -> dict[str, str]:
    titles: dict[str, str] = {}
    reader = csv.DictReader(io.StringIO(raw), delimiter="\t")
    for row in reader:
        serial = (row.get("serial") or row.get("ID") or "").replace("-", "").replace(" ", "").upper()
        title = (row.get("title") or "").strip()
        if not serial or not title:
            continue
        if BAD_URL.search(title):
            continue
        title = re.sub(r"\s+", " ", title.translate(TRADEMARK)).strip()
        if title and serial not in titles:
            titles[serial] = title
    return titles


def main() -> None:
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_OUT
    titles = parse(fetch())
    out.parent.mkdir(parents=True, exist_ok=True)
    with open(out, "w", encoding="utf-8") as f:
        for serial in sorted(titles):
            f.write(f"{serial}\t{titles[serial]}\n")
    print(f"wrote {out} ({len(titles)} titles)")


if __name__ == "__main__":
    main()
