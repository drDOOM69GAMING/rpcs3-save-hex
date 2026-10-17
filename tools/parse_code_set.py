#!/usr/bin/env python3
"""Parse the scraped community PS3 code pages into a code database.

The scraper saves one HTML page per game serial. Only the `span.index_menu`
block containing the `CODE` marker is read, so HTML from the rest of the page
cannot leak into a cheat.

The pages use a small set of markers:

* `:FILE`               the save file the cheats that follow target
* `group:X\\Y`, `[group:X\\Y]`, `[GROUP:X\\Y]`
                        a section heading; the cheats that follow live under it
* `[Cheat name]`        a cheat, followed by its code lines
* `[Something (Required)]` and `[default:Something]`
                        a step that runs with the cheats of the same file
                        (a checksum fix, an unpack step). It is folded into
                        those cheats rather than offered as a cheat of its own.
* `;comment`            a comment, ignorable

Usage:
    python3 tools/parse_code_set.py [PAGES_DIR] [OUTPUT_JSON]

Defaults: PAGES_DIR=/tmp/ps3_code_pages, OUTPUT_JSON=assets/codes.json
"""
import fnmatch
import json
import re
import sys
from pathlib import Path

from bs4 import BeautifulSoup

PAGES_DIR = Path(sys.argv[1]) if len(sys.argv) > 1 else Path('/tmp/ps3_code_pages')
OUTPUT_DB = Path(sys.argv[2]) if len(sys.argv) > 2 else Path(__file__).resolve().parent.parent / 'assets' / 'codes.json'

TAG = re.compile(r'<[^>]+>')
BR = re.compile(r'<\s*br\s*/?\s*>', re.I)
SERIAL_LIKE = re.compile(r'^[A-Z]{2,5}\d{3,}$')
# Never let a page's own branding or a URL become a game name.
SITE_LINE = re.compile(
    r'save[\s\-_]*editor|ps3\s+save\s+editor|'
    r'https?://|www\.|\.com\b|\.net\b|\.org\b',
    re.I,
)

# A section heading, with or without brackets, in any letter case.
GROUP_LINE = re.compile(r'^\[?\s*group\s*:\s*(.*?)\s*\]?$', re.I)
# A step that runs automatically with the cheats of its file.
STEP_LINE = re.compile(r'^\[?\s*default\s*:\s*(.*?)\s*\]?$', re.I)
# A `(Required)` / `(Must Be On)` block is a step, not a cheat.
REQUIRED = re.compile(r'\(\s*(?:required|must be on)\s*\)', re.I)
BLOCK = re.compile(r'^\[(.+?)\]$')

# A coded line: an 8-digit address and an 8-digit value.
SW_LINE = re.compile(r'^([0-9A-Fa-f]{8})\s+([0-9A-Fa-f]{8})(?:\s.*)?$')


def code_block_lines(raw: str):
    """Return the code-block lines for a page, or None when it has no block."""
    soup = BeautifulSoup(raw, 'html.parser')
    for span in soup.find_all('span', class_='index_menu'):
        inner = str(span)
        if '<b>CODE</b>' not in inner:
            continue
        inner = inner.split('<b>CODE</b>', 1)[1]
        inner = BR.sub('\n', inner)
        inner = TAG.sub('', inner)
        import html as _html
        inner = _html.unescape(inner)
        out = []
        for line in inner.splitlines():
            line = re.sub(r'[\ufffd\u0000-\u001f]', '', line).strip().strip('\ufeff')
            if line and '<' not in line and '>' not in line:
                out.append(line)
        return out
    return None


def strip_comment(line: str) -> str:
    """Drop a trailing `;comment`, leaving `;` inside quoted keys alone."""
    out = []
    quoted = False
    for ch in line:
        if ch == '"':
            quoted = not quoted
        elif ch == ';' and not quoted:
            break
        out.append(ch)
    return ''.join(out).strip()


def _is_serial_isn(body: str, serial: str) -> bool:
    if re.fullmatch(re.escape(serial), body, re.I):
        return True
    stripped = re.sub(r'\(psn\)\s*$', '', body, flags=re.I).strip()
    return bool(re.fullmatch(r'[A-Z]{2,5}\d{3,}', stripped, re.I))


def _is_site_line(body: str) -> bool:
    return bool(re.search(
        r'save[\s\-_]*editor|save[\s\-_]*wizard|https?://|www\.|\.(?:com|net|org)\b',
        body, re.I))


def _base_name(pattern: str) -> str:
    """The file part of a `:` target, lower-cased (`folder\\file` -> `file`)."""
    return re.split(r'[\\/]', pattern or '')[-1].lower()


def compatible(target_a: str, target_b: str) -> bool:
    """True when two `:` targets can name the same physical file.

    A missing target matches anything; otherwise the file parts must match as
    wildcards in either direction (`USR-DATA` matches `BCUS...\\USR-DATA` and
    `game.00*` matches `game.000`).
    """
    a, b = _base_name(target_a), _base_name(target_b)
    if not a or not b:
        return True
    return fnmatch.fnmatchcase(a, b) or fnmatch.fnmatchcase(b, a)


def game_name_from(lines, serial):
    """Pick the game name from the page's `;` comments."""
    candidates = []
    for i, l in enumerate(lines):
        if re.fullmatch(r';\s*' + re.escape(serial) + r'\s*', l, re.I):
            for off in (-1, 1, -2, 2, -3, 3):
                j = i + off
                if 0 <= j < len(lines) and lines[j].startswith(';'):
                    candidates.append(lines[j])
    candidates.extend(l for l in lines if l.startswith(';'))

    for l in candidates:
        body = strip_comment(l[1:].strip())
        if not body:
            continue
        if body.lower().startswith('from') or body.startswith('---') or body.endswith('---'):
            continue
        if _is_serial_isn(body, serial):
            continue
        if _is_site_line(body):
            continue
        return body
    return serial


def parse_entries(lines):
    """Split a page's code block into cheat and step entries, in order."""
    entries = []
    state = {'file': '', 'group': '', 'name': None, 'lines': [], 'step': False}

    def set_group(title):
        """Store a section heading, turning `Parent - Child` into a path."""
        state['group'] = title.replace(' - ', '\\').strip('\\/ ')

    def flush():
        if state['name'] is None:
            state['lines'] = []
            return
        entries.append({
            'kind': 'step' if state['step'] else 'cheat',
            'file': state['file'],
            'group': state['group'],
            'name': state['name'],
            'lines': list(state['lines']),
        })
        state['lines'] = []
        state['name'] = None

    for raw in lines:
        if raw.startswith(';'):
            # Some pages use a dashed comment as a section heading, e.g.
            # `; --- Save Slot 1 - Cao Cao ---`, instead of a `group:` line.
            h = re.match(r'^\s*[-=_*]{3,}\s*(.*?)\s*[-=_*]{3,}\s*$', raw[1:])
            if h and h.group(1).strip():
                flush()
                set_group(h.group(1).strip())
            continue
        line = strip_comment(raw)
        if not line:
            continue
        if line.startswith(':') and not line.startswith('::'):
            flush()
            state['file'] = line[1:].strip()
            continue
        m = BLOCK.match(line)
        if m:
            flush()
            nm = m.group(1).strip()
            gm = re.fullmatch(r'group\s*:\s*(.*)', nm, re.I)
            if gm:
                state['group'] = gm.group(1).strip().strip('\\/').strip()
                continue
            dm = re.fullmatch(r'default\s*:\s*(.*)', nm, re.I)
            if dm:
                state['name'] = dm.group(1).strip()
                state['step'] = True
                continue
            state['name'] = nm
            state['step'] = bool(re.search(r'\(required\)|\(must be on\)', nm, re.I))
            continue
        gm = re.fullmatch(r'group\s*:\s*(.*)', line, re.I)
        if gm:
            flush()
            state['group'] = gm.group(1).strip().strip('\\/').strip()
            continue
        dm = re.fullmatch(r'default\s*:\s*(.*)', line, re.I)
        if dm:
            flush()
            state['name'] = dm.group(1).strip()
            state['step'] = True
            continue
        if state['name'] is not None:
            state['lines'].append(line)

    flush()
    return entries


def fold_steps(entries):
    """Fold each `(Required)` / `default:` step into the cheats it runs with.

    A step appears after the cheats of its own save file, so it is appended to
    every cheat gathered for that file since the previous step. A step with no
    cheats of its own file (a page that lists a file's checksum but has no
    cheats for it) is dropped.
    """
    cheats = []
    pending = []
    for e in entries:
        if e['kind'] == 'cheat':
            if pending and not compatible(pending[-1]['file'], e['file']):
                pending = []
            cheats.append(e)
            pending.append(e)
        else:
            if pending and compatible(pending[0]['file'], e['file']):
                for p in pending:
                    p['lines'].extend(e['lines'])
            pending = []
    return cheats


def main():
    games = []
    total = 0
    steps = 0

    for page in sorted(PAGES_DIR.glob('*.html')):
        serial = page.stem
        raw = page.read_text(encoding='utf-8', errors='ignore')
        lines = code_block_lines(raw)
        if lines is None:
            continue

        name = game_name_from(lines, serial)
        entries = parse_entries(lines)
        steps += sum(1 for e in entries if e['kind'] == 'step')

        codes = []
        for e in fold_steps(entries):
            offset = value = ''
            for cl in e['lines']:
                m = SW_LINE.match(cl)
                if m:
                    offset = '0x' + m.group(1).upper()
                    value = '0x' + m.group(2).upper()
                    break
            codes.append({
                'name': e['name'],
                'group': e['group'],
                'file': e['file'],
                'offset': offset,
                'value': value,
                'lines': list(e['lines']),
            })

        games.append({'serial': serial, 'title_id': serial, 'name': name, 'codes': codes})

    total = sum(len(g['codes']) for g in games)
    db = {'source': 'community', 'games': games, 'total_games': len(games), 'total_codes': total}
    OUTPUT_DB.parent.mkdir(parents=True, exist_ok=True)
    with open(OUTPUT_DB, 'w', encoding='utf-8') as f:
        json.dump(db, f, indent=2, ensure_ascii=False)

    unnamed = [g['serial'] for g in games if not g['name'] or g['name'] == g['serial']]
    print(f'games {len(games)}  cheats {total}')
    print(f'required/default steps folded in: {steps}')
    print(f'games with no name: {len(unnamed)} {unnamed}')


if __name__ == '__main__':
    main()
