#!/usr/bin/env python3
"""Merge the larger patch database into `assets/codes.json`.

That database is a much larger set than the bundled community code list: around
1,800 PS3 games in the same code language this tool interprets. We import the
games the bundled database does not already cover; titles that both sources list
keep the existing codes so nothing is duplicated.

Patches that need an embedded Python script, a game-specific cipher or a
compression step are imported too, but each of their codes is marked
`unsupported` with a reason, so the GUI lists the cheat and explains why it
cannot be applied instead of corrupting the save.

Each patch file marks the steps that must run around every cheat with a
`(Required)` suffix, e.g. `[Decrypt RAGE.SAV (Required)]` before the cheats and
`[Update CHKS checksum (Required)]` / `[Encrypt RAGE.SAV (Required)]` after
them. The source database auto-adds those when a code is selected, so we bake
them into each code's own lines: the steps above the cheat go first, the ones
below go last, and the `(Required)` blocks themselves are not offered as cheats.

Usage:
    python3 tools/merge_code_db.py [PATCHES_DIR] [CODES_JSON]

Defaults:
    PATCHES_DIR = /tmp/opencode/patch-db/PS3
    CODES_JSON  = assets/codes.json

Point PATCHES_DIR at the PS3 folder of a checkout of the patch database.
"""

import fnmatch
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PATCHES_DIR = Path(sys.argv[1]) if len(sys.argv) > 1 else Path('/tmp/opencode/patch-db/PS3')
CODES_JSON = Path(sys.argv[2]) if len(sys.argv) > 2 else ROOT / 'assets' / 'codes.json'
TITLES_TSV = ROOT / 'assets' / 'titles.tsv'

SERIAL_LINE = re.compile(r'^[A-Z]{2,5}\d{3,}$')
CREDIT = re.compile(r'^(from|by|codes? by|all codes|credits?|thanks?|originally)\b', re.I)
SITE = re.compile(r'save[\s\-_]*editor|save[\s\-_]*wizard|https?://|www\.|\.(?:com|net|org)\b', re.I)
GROUP_BRACKET = re.compile(r'^\[Group:\s*(.+?)\]\s*$', re.I)
GROUP_PLAIN = re.compile(r'^Group:\s*(.+)$', re.I)
BLOCK = re.compile(r'^\[(.+?)\]\s*$')
MENU_DEF = re.compile(r'^\{[A-Za-z0-9_]+\}.*\{/[A-Za-z0-9_]+\}$')
PLACEHOLDER = re.compile(r'\{[A-Za-z0-9_]+\}')
PYTHON_BLOCK = re.compile(r'^python:', re.I)
# A block that is an automatic step rather than a selectable cheat: the sources
# spell it `(Required)`, `(must be on)` or a `[default:...]` heading.
REQUIRED_NAME = re.compile(r'\(required\)|\(must be on\)|^\s*default\s*:', re.I)
# A dashed comment used as a section heading, e.g. `; --- Save Slot 1 - Cao Cao ---`.
DASHED_HEADING = re.compile(r'^\s*[-=_*]{3,}\s*(.*?)\s*[-=_*]{3,}\s*$')
CRYPT_CMD = re.compile(r'^(?:decrypt|encrypt)\s+([A-Za-z0-9_]+)', re.I)
PRE_CMD = re.compile(r'^(?:decrypt|decompress|uncompress)\b', re.I)
COMPRESS_CMD = re.compile(r'^(?:de|un)?compress\b', re.I)
# `set [name]:<function>` picks the checksum a code wants.
SET_FN = re.compile(r'^set\s+\[[^\]]*\]\s*:\s*([A-Za-z_][A-Za-z0-9_]*)', re.I)
# Checksums the corpus knows about but this port does not implement. Keep this
# in step with `UNSUPPORTED_FNS` in `src/engine/bsd.rs`; a code that needs one of
# them is marked unsupported rather than writing a wrong value.
UNSUPPORTED_FNS = (
    'md2',
    'adler16',
    'fletcher16',
    'fletcher32',
    'dbzxv2_checksum',
    'murmur3_32',
    'jhash',
    'mgs2_checksum',
    'mgspw_checksum',
    'ff13_checksum',
    'deadrising_checksum',
    'lookup3',
    'host_',
)
SW_LINE = re.compile(r'^([0-9A-Fa-f]{8})\s+([0-9A-Fa-f]{8})$')
# A coded line, and the looser shapes the community listings use for one: a
# short address/value, or a field that holds a placeholder letter instead of a
# digit (`20000289 xxxx0000`).
SW_LOOSE = re.compile(r'^(\S{1,8})\s+(\S{1,8})$')
HEX8 = re.compile(r'^[0-9A-F]{8}$')
CHOICE8 = re.compile(r'^[0-9A-Z?*]{8}$')
TRADEMARK = str.maketrans('', '', '\u2122\u00ae\u00a9')

# The ciphers `src/engine/crypto.rs` implements. Anything else named by a cheat
# is a game-specific scheme and is marked unsupported rather than guessed at.
SUPPORTED_CIPHERS = {
    'aes_ecb',
    'aes_cbc',
    'blowfish_ecb',
    'blowfish_cbc',
    'des3_ecb',
    'des3_cbc',
}

UNSUPPORTED_PYTHON = (
    'This cheat needs a Python script that this tool cannot run, so it is '
    'listed here but cannot be applied.'
)
UNSUPPORTED_CRYPTO = (
    'This save uses the game\'s own encryption, which this tool cannot decrypt, '
    'so the cheat is listed here but cannot be applied.'
)
UNSUPPORTED_COMPRESS = (
    'This save is compressed, which this tool cannot unpack, so the cheat is '
    'listed here but cannot be applied.'
)
UNSUPPORTED_HASH = (
    'This save uses a game-specific checksum that this tool cannot compute, so '
    'the cheat is listed here but cannot be applied.'
)
UNSUPPORTED_CHOICE = (
    'This cheat needs you to choose a value, which you can enter next to the '
    'code before applying it.'
)
UNSUPPORTED_INCOMPLETE = (
    'This listing is only part of a cheat: it reads a value the code never '
    'sets, so it cannot be applied on its own.'
)
UNSUPPORTED_TRUNCATED = (
    'This listing is incomplete: a multi-write command is missing its second '
    'line, so it cannot be applied.'
)
UNSUPPORTED_BITWISE = (
    'This listing is malformed: it changes a value that it never sets, so it '
    'cannot be applied.'
)

# Codes the interpreter refuses, recorded once by running the engine over an
# unflagged database (`examples/audit.rs` reports them). The merge marks them
# unsupported so the UI can show a plain reason instead of a raw interpreter
# error. The engine stays the authority: a code that cannot run is never applied.
STRUCTURAL_OVERRIDES = ROOT / 'tools' / 'unsupported_structural.json'
STRUCTURAL_REASONS = {
    'truncated': UNSUPPORTED_TRUNCATED,
    'bitwise': UNSUPPORTED_BITWISE,
}

# The community pages spell the plain CRC-32 hash `crc-32` in a couple of
# places. The interpreter only knows `crc32`; `crc-32` would fall through to the
# generic `crc*` handler with its default (all zero) parameters and write the
# wrong checksum, so normalise it to the name the engine understands.
HASH_TYPO = re.compile(r'(\]:\s*)crc-32\b', re.I)

# Zero-width and byte-order marks that leak out of the scraped pages, plus the
# non-breaking space that should read as an ordinary space.
ZERO_WIDTH = re.compile(r'[\ufeff\u200b\u200c\u200d\u200e\u200f]')

# The value a `set [name]:...` names, and any `[name]` a line refers to.
SET_VAR = re.compile(r'^set\s+\[([^\[\]]+)\]\s*:', re.I)
VAR_REF = re.compile(r'\[([^\[\]]+)\]')

# A page sometimes drops the colon in `write at <offset>:<value>`, e.g.
# `write at 0x4EBA5[hash]` or `write at 0x10A2569 0x63`. The reference tool
# treats the missing colon as a crash; drop the colon back in so the write
# happens instead of being reported as an unrecognised line.
WRITE_FIX = re.compile(r'^(write\s+(?:at|next)\s+[0-9A-Fa-fxX()]+)\s*(?=\[|"|0x)', re.I)

# A `write` line whose variable reference lost its closing bracket.
WRITE_OPEN_VAR = re.compile(r'^write\s+.*\[[^\]]*$', re.I)

# A `search` line whose hex pattern has a space inside it, e.g.
# `search 0x7765...7368 6F74...`. The space splits the pattern and the
# reference cannot decode it, so the stray whitespace is dropped.
SEARCH_HEX = re.compile(r'^(search\s+(?:next\s+)?0x)([0-9A-Fa-f\s]+?)(:\d+)?$', re.I)

# The first word of every command the interpreter understands. Anything else in
# a code body is page wording (a heading, a note, a stray label) that belongs in
# a section title, not mixed in with the codes.
SCRIPT_LINE = re.compile(
    r'^(?:carry|set|write|insert|delete|search|copy|msgbox|endian_swap|'
    r'decrypt|encrypt|decompress|compress)\b',
    re.I,
)

# Colour prefixes and `INFO:` markers that some listings put in front of a name.
NAME_COLOUR = re.compile(r'^\s*(?:red|blue|green|yellow|orange|purple|grey|gray|info|note|default)\s*:\s*', re.I)


def strip_comment(line: str) -> str:
    """Drop a trailing `;comment`, leaving `;` inside a quoted HMAC key alone."""
    out = []
    quoted = False
    for ch in line:
        if ch == '"':
            quoted = not quoted
        elif ch == ';' and not quoted:
            break
        out.append(ch)
    return ''.join(out).strip()


def fix_write_colon(line: str) -> str:
    """Re-insert a dropped `:` in a `write at <offset>:<value>` line."""
    if ':' in line:
        return line
    m = WRITE_FIX.match(line)
    if not m:
        return line
    return f'{m.group(1)}:{line[m.end(1):].lstrip()}'


def fix_open_var(line: str) -> str:
    """Close a variable reference the source left open in a `write` line.

    Both sources carry `write at 0x000000:[hash` for one game, where the closing
    bracket was lost in the listing.
    """
    if line.count('[') == line.count(']') + 1 and WRITE_OPEN_VAR.match(line):
        return line + ']'
    return line


def clean_line(raw: str):
    """Turn one stored line into (line, needs_choice), or (None, _) to drop it.

    Page wording (a stray heading that leaked into a code body) is dropped, a
    line that is only a placeholder such as `20000289 xxxx0000` is reported as
    needing a choice, and a short coded line such as `2000008 00000000` is
    padded back to its full width.
    """
    line = strip_comment(raw)
    line = ZERO_WIDTH.sub('', line).replace('\u00a0', ' ').strip()
    if not line:
        return None, False
    line = HASH_TYPO.sub(r'\1crc32', line)
    m = SEARCH_HEX.match(line)
    if m:
        line = m.group(1) + re.sub(r'\s+', '', m.group(2)) + (m.group(3) or '')
    line = fix_write_colon(line)
    line = fix_open_var(line)
    if SW_LINE.match(line) or SCRIPT_LINE.match(line):
        return line, False
    # A coded line can carry a stray closing bracket from the page, e.g.
    # `0000002B 00000001]`. Only a coded line is repaired this way: a command
    # such as `write at 0x0:[crc]` keeps its brackets.
    line = re.sub(r'[\]\)]\s*$', '', line).strip()
    if not line:
        return None, False
    if SW_LINE.match(line) or SCRIPT_LINE.match(line):
        return line, False
    m = SW_LOOSE.match(line)
    if m:
        a = m.group(1).upper().rjust(8, '0')
        b = m.group(2).upper().rjust(8, '0')
        if HEX8.match(a) and HEX8.match(b):
            return f'{a} {b}', False
        if CHOICE8.match(a) and CHOICE8.match(b):
            return line, True
    return None, False


def clean_codes(db):
    """Normalise every code body and flag cheats that need a chosen value."""
    dropped = flagged = 0
    for game in db['games']:
        kept = []
        for code in game['codes']:
            needs_choice = False

            def clean_block(raw_lines):
                out = []
                for raw in raw_lines:
                    line, choice = clean_line(raw)
                    if line is None:
                        continue
                    if choice:
                        nonlocal needs_choice
                        needs_choice = True
                    out.append(line)
                return out

            lines = clean_block(code.get('lines', []))
            steps = []
            for step in code.get('steps', []):
                block = clean_block(step.get('lines', []))
                if block:
                    steps.append({'file': step.get('file', ''), 'lines': block})

            if steps:
                # A multi-file cheat keeps its own writes in `lines` and its
                # full program in `steps`; both must survive normalisation.
                program = flat_lines([(s['file'], s['lines']) for s in steps])
                if not program:
                    dropped += 1
                    continue
                code['steps'] = steps
            else:
                code['steps'] = []
                if not lines:
                    dropped += 1
                    continue
                code['lines'] = lines
                program = lines
            code['lines'] = lines
            if needs_choice and not code.get('unsupported'):
                code['unsupported'] = UNSUPPORTED_CHOICE
                flagged += 1
            if not code.get('unsupported'):
                reason = classify(program, False)
                if reason:
                    code['unsupported'] = reason
                    flagged += 1
            code['name'] = clean_name(NAME_COLOUR.sub('', code['name']))
            code['group'] = code.get('group', '').strip().strip('\\/ ').replace('/', '\\')
            offset = value = ''
            for cl in lines:
                m = SW_LINE.match(cl)
                if m:
                    offset = '0x' + m.group(1).upper()
                    value = '0x' + m.group(2).upper()
                    break
            code['offset'] = offset
            code['value'] = value
            kept.append(code)
        game['codes'] = kept
    db['total_codes'] = sum(len(g['codes']) for g in db['games'])
    return dropped, flagged



def clean_name(raw: str) -> str:
    return re.sub(r'\s+', ' ', raw.translate(TRADEMARK)).strip()


def base_name(pattern: str) -> str:
    """The file part of a `:` target, lower-cased (`folder\\file` -> `file`)."""
    return re.split(r'[\\/]', pattern)[-1].lower()


def compatible(target_a: str, target_b: str) -> bool:
    """True when two `:` targets can name the same physical file.

    A missing target matches anything; otherwise the file parts must match as
    wildcards in either direction (`USR-DATA` matches `BCUS...\\USR-DATA` and
    `game.00*` matches `game.000`).
    """
    a, b = base_name(target_a), base_name(target_b)
    if not a or not b:
        return True
    return fnmatch.fnmatchcase(a, b) or fnmatch.fnmatchcase(b, a)


def _norm_hash(line: str) -> str:
    return HASH_TYPO.sub(r'\1crc32', line.strip())


def _already_has(have, want) -> bool:
    """True when every line of `want` is already present in `have`.

    Compared with the same hash normalisation `clean_line` applies, so a source
    that spelled the hash `crc-32` still counts as already carrying the step and
    it is not folded in a second time.
    """
    have_n = {_norm_hash(line) for line in have}
    return all(_norm_hash(line) in have_n for line in want)


def _is_unsupported_fn(value: str) -> bool:
    low = value.lower()
    return any(low.startswith(name) for name in UNSUPPORTED_FNS)


def undefined_vars(code_lines) -> bool:
    """True when a code reads a `[name]` value it never assigns.

    A listing that only has the `write at ...:[hash]` half of a checksum fix
    (the other half lives on another page) cannot be applied on its own, so it
    is flagged instead of silently writing nothing.
    """
    assigned = set()
    refs = []
    for line in code_lines:
        text = line.strip()
        m = SET_VAR.match(text)
        if m:
            assigned.add(m.group(1).strip().lower())
            rest = text[m.end():]
        else:
            rest = text
        refs.extend(name.strip().lower() for name in VAR_REF.findall(rest))
    return any(name not in assigned for name in refs)


def block_vars(lines):
    """The variable names a block assigns and the ones it reads.

    `set [name]:value` assigns; every other `[name]` in the line reads. Names
    are lower-cased because the interpreter matches them case-insensitively.
    """
    assigned, refs = set(), set()
    for line in lines:
        text = line.strip()
        m = SET_VAR.match(text)
        if m:
            assigned.add(m.group(1).strip().lower())
            rest = text[m.end():]
        else:
            rest = text
        refs.update(n.strip().lower() for n in VAR_REF.findall(rest))
    return assigned, refs


def build_program(file, own_lines, required_blocks):
    """Order the `(Required)` steps a code needs around its own lines.

    A step joins the code when it targets the same file or when it shares a
    variable with the code: it either supplies a value the code reads, or it
    consumes one the code writes. That is what pulls a checksum chain across
    files whole — the block that seeds the checksum, the one that computes it
    over the cheat's file and the one that stores it back are linked by
    `[crc_init]`/`[hash]`, so all three are needed before any of them can run.

    Decrypt/unpack steps run first, everything else after the cheat, and within
    each group the patch's own order is kept so a seed is always set before it
    is read.

    Returns `(steps, needs_python)` where `steps` is an ordered list of
    `(file, lines)` with runs on the same file already merged together.
    """
    own_assigned, own_refs = block_vars(own_lines)
    chosen = []
    assigned, refs = set(own_assigned), set(own_refs)
    grew = True
    while grew:
        grew = False
        for i, r in enumerate(required_blocks):
            if i in chosen or r['python']:
                continue
            a, f = block_vars(r['lines'])
            if compatible(file, r['file']) or (a & refs) or (f & assigned):
                chosen.append(i)
                assigned |= a
                refs |= f
                grew = True

    ordered = sorted(chosen, key=lambda i: i)
    pre = [required_blocks[i] for i in ordered if required_blocks[i]['kind'] == 'pre']
    post = [required_blocks[i] for i in ordered if required_blocks[i]['kind'] != 'pre']
    needs_python = any(required_blocks[i]['python'] for i in chosen)

    parts = [(r['file'], r['lines']) for r in pre]
    parts.append((file, list(own_lines)))
    parts.extend((r['file'], r['lines']) for r in post)

    steps = []
    for target, block in parts:
        if steps and steps[-1][0] == target:
            steps[-1][1].extend(block)
        else:
            steps.append((target, list(block)))
    return steps, needs_python


def flat_lines(steps):
    """Every step's lines concatenated, for validation and single-file codes."""
    out = []
    for _target, block in steps:
        out.extend(block)
    return out


def program_lines(steps):
    """The steps in the `:FILE` form a multi-file interpreter run uses."""
    out = []
    for target, block in steps:
        if target:
            out.append(f':{base_name(target)}')
        out.extend(block)
    return out


def classify(code_lines, needs_python: bool):
    """Why a code cannot be applied, or None when it can."""
    if needs_python:
        return UNSUPPORTED_PYTHON
    for line in code_lines:
        text = line.strip()
        m = CRYPT_CMD.match(text)
        if m and m.group(1).lower() not in SUPPORTED_CIPHERS:
            return UNSUPPORTED_CRYPTO
        if COMPRESS_CMD.match(text):
            return UNSUPPORTED_COMPRESS
        h = SET_FN.match(text)
        if h and _is_unsupported_fn(h.group(1)):
            return UNSUPPORTED_HASH
    if any(PLACEHOLDER.search(line) for line in code_lines):
        return UNSUPPORTED_CHOICE
    if undefined_vars(code_lines):
        return UNSUPPORTED_INCOMPLETE
    return None


def game_name_from(lines):
    """The game name is the first `;` comment that is not a serial or a credit."""
    for line in lines:
        stripped = line.strip()
        if not stripped:
            continue
        if not stripped.startswith(';'):
            break
        body = stripped[1:].strip()
        if not body:
            continue
        if SERIAL_LINE.match(body) or SITE.search(body) or CREDIT.match(body):
            continue
        return clean_name(body)
    return ''


def parse_patch(path: Path):
    """Return (name, codes) for one `.savepatch` file.

    The blocks are collected in file order first, then each cheat gets the
    `(Required)` blocks that can name the same file folded into its own lines:
    the ones above it first, the ones below it last. That turns a code that
    relied on the source database auto-adding a decrypt / checksum / encrypt step
    into a code this tool can apply on its own.
    """
    lines = path.read_text(encoding='utf-8', errors='ignore').splitlines()

    name = game_name_from(lines)

    entries = []
    file = group = ''
    current = None

    def flush():
        nonlocal current
        if current is not None:
            entries.append(current)
            current = None

    for line in lines:
        stripped = line.strip()
        if not stripped:
            continue
        if stripped.startswith(';'):
            # A dashed comment is sometimes the section heading, the same way
            # the scraped pages write `; --- Save Slot 1 - Cao Cao ---`.
            h = DASHED_HEADING.match(stripped[1:])
            if h and h.group(1).strip():
                flush()
                group = h.group(1).replace(' - ', '\\').strip('\\/ ')
            continue
        if MENU_DEF.match(stripped):
            continue  # {ST} / {PNTS} menu definition, not a cheat
        if stripped.startswith(':'):
            flush()
            file = stripped[1:].strip()
            continue
        m = GROUP_BRACKET.match(stripped) or GROUP_PLAIN.match(stripped)
        if m:
            flush()
            group = m.group(1).strip()
            continue
        m = BLOCK.match(stripped)
        if m:
            flush()
            block = m.group(1).strip()
            if block.upper().startswith('GROUP'):
                continue
            current = {
                'file': file,
                'group': group,
                'name': block,
                'lines': [],
                'python': bool(PYTHON_BLOCK.match(block)),
            }
            continue
        if current is not None:
            current['lines'].append(stripped)

    flush()

    required = [i for i, e in enumerate(entries) if REQUIRED_NAME.search(e['name'])]

    # A required step runs before the cheat when it prepares the data (decrypt /
    # unpack) and after it when it finalises it (checksum / re-encrypt). The
    # in-file position decides for the patch's own codes; this classification is
    # what lets the steps be folded into another source's codes too.
    required_blocks = []
    for j in required:
        e = entries[j]
        required_blocks.append(
            {
                'file': e['file'],
                'lines': e['lines'],
                'python': e['python'],
                'kind': 'pre'
                if any(PRE_CMD.match(l.strip()) for l in e['lines'])
                else 'post',
            }
        )

    codes = []
    for i, entry in enumerate(entries):
        if REQUIRED_NAME.search(entry['name']) or entry['python']:
            continue  # not a selectable cheat

        steps, needs_python = build_program(
            entry['file'], entry['lines'], required_blocks
        )
        own = list(entry['lines'])
        if not own:
            continue  # an info-only heading, nothing to apply

        offset = value = ''
        for cl in entry['lines']:
            m = SW_LINE.match(cl)
            if m:
                offset = '0x' + m.group(1).upper()
                value = '0x' + m.group(2).upper()
                break

        code = {
            'name': entry['name'],
            'group': entry['group'],
            'file': entry['file'],
            'offset': offset,
            'value': value,
            'lines': own,
        }
        targets = {base_name(t) for t, _ in steps if t}
        if len(targets) > 1:
            # The work spans several files, so it is stored as ordered per-file
            # steps the interpreter switches between.
            code['steps'] = [
                {'file': t, 'lines': ls} for t, ls in steps if ls
            ]
        else:
            code['lines'] = flat_lines(steps)
        reason = classify(flat_lines(steps), needs_python)
        if reason:
            code['unsupported'] = reason
        codes.append(code)

    return name, codes, required_blocks


def fold_required(code, required_blocks):
    """Fold matching `(Required)` steps into a code that came from elsewhere.

    Used for the codes the bundled community database already had for a game
    that an imported patch also covers, so those codes get the same decrypt /
    checksum / encrypt steps instead of writing to an encrypted save. A step that
    the code already carries is not added a second time.
    """
    if not code.get('file'):
        return  # no target file, so there is nothing to decrypt
    if code.get('steps'):
        return  # already split into per-file steps
    if any(CRYPT_CMD.match(l.strip()) or COMPRESS_CMD.match(l.strip()) for l in code['lines']):
        return  # the code already handles its own crypto

    blocks = [r for r in required_blocks if not _already_has(code['lines'], r['lines'])]
    steps, needs_python = build_program(code['file'], code['lines'], blocks)

    targets = {base_name(t) for t, _ in steps if t}
    if len(targets) > 1:
        code['steps'] = [{'file': t, 'lines': ls} for t, ls in steps if ls]
    else:
        code['lines'] = flat_lines(steps)
    if code['steps'] or code['lines']:
        reason = classify(flat_lines(steps), needs_python)
        if reason:
            code['unsupported'] = reason
    elif needs_python:
        code['unsupported'] = UNSUPPORTED_PYTHON


HMAC_STAR = re.compile(r'hmac_sha1\(\*\)', re.I)
HMAC_KEY = re.compile(r'hmac_sha1\((".*?")\)', re.I)

# Website addresses and site branding the bundled code lists sometimes carry in
# a code's name. They are stripped so nothing shown in the app advertises a site.
NAME_URL = re.compile(r'\s*(?:https?://|www\.)\S+', re.I)
NAME_BRAND = re.compile(
    r'\s*(?:save[\s\-_]*editor|save[\s\-_]*wizard|ps3cheating\.net|savedata\.jp|'
    r'docs\.google\.com)\S*',
    re.I,
)
NAME_SITE = re.compile(
    r'https?://|www\.|save[\s\-_]*editor|save[\s\-_]*wizard|ps3cheating\.net|'
    r'savedata\.jp|docs\.google\.com',
    re.I,
)
NAME_INFO = re.compile(r'^\s*(?:RED\s*:\s*)?INFO\s*:?\s*', re.I)


def clean_code_names(db):
    """Drop website addresses and branding from cheat names.

    A handful of the bundled entries are notes rather than real cheats, named
    with the address of the site that posted them. Anything that is only a note
    (no lines to apply) is removed; a real code keeps its name with the address
    stripped.
    """
    dropped = renamed = 0
    for game in db['games']:
        kept = []
        for code in game['codes']:
            original = code['name']
            if not NAME_SITE.search(original):
                kept.append(code)
                continue
            name = NAME_INFO.sub('', NAME_BRAND.sub(' ', NAME_URL.sub(' ', original)))
            name = re.sub(r'\s+', ' ', name).strip(' -:,;!*#')
            if not name:
                if not code['lines']:
                    dropped += 1
                    continue
                name = 'Save fix'
            if name != original:
                renamed += 1
                code['name'] = name
            kept.append(code)
        game['codes'] = kept
    db['total_codes'] = sum(len(g['codes']) for g in db['games'])
    return dropped, renamed


def code_signature(code):
    """What makes two listings the same cheat: its target and its whole program."""
    steps = code.get('steps') or []
    if steps:
        program = tuple(
            (base_name(s.get('file', '')), tuple(line.strip() for line in s.get('lines', [])))
            for s in steps
        )
    else:
        program = tuple(line.strip() for line in code.get('lines', []))
    return (code.get('file', ''), program)


def dedupe_codes(db):
    """Remove exact duplicate cheats that the two sources both listed.

    Each source can carry the same cheat, and the two only become identical once
    the bodies are normalised (a hash spelling fixed, a redacted HMAC key
    filled). Keeping one copy avoids showing the same cheat twice; when the
    duplicate carries a group the kept copy does not, the group is inherited.
    """
    removed = 0
    for game in db['games']:
        kept = []
        index = {}
        for code in game['codes']:
            key = code_signature(code)
            if key in index:
                prev = kept[index[key]]
                if not prev.get('group') and code.get('group'):
                    prev['group'] = code['group']
                removed += 1
                continue
            index[key] = len(kept)
            kept.append(code)
        game['codes'] = kept
    db['total_codes'] = sum(len(g['codes']) for g in db['games'])
    return removed


def fill_hmac_keys(db):
    """Spell out the HMAC key where the bundled code list hides it as `*`.

    A few saves (the Uncharted / The Last of Us family) list their HMAC step as
    `set [hash]:hmac_sha1(*)`, because the code listing redacts the key. The
    patch for the same file spells the key out, and the import folds that step
    into the same code, so the key is right there to reuse.
    """
    filled = 0
    for game in db['games']:
        for code in game['codes']:
            if not any(HMAC_STAR.search(l) for l in code['lines']):
                continue
            key = None
            for line in code['lines']:
                m = HMAC_KEY.search(line)
                if m:
                    key = m.group(1)
                    break
            if not key:
                continue
            code['lines'] = [HMAC_STAR.sub(f'hmac_sha1({key})', l) for l in code['lines']]
            filled += 1
    return filled


def apply_structural_overrides(db, path=STRUCTURAL_OVERRIDES):
    """Mark the codes the interpreter refuses as unsupported, with a reason.

    The list is produced by running the engine itself, so it matches the
    runtime exactly instead of re-deriving the hex grammar in Python.
    """
    if not path.exists():
        return 0, []
    entries = json.loads(path.read_text(encoding='utf-8'))
    wanted = {(e['serial'], e.get('file', ''), e['name']): e['kind'] for e in entries}
    hit = set()
    applied = 0
    for game in db['games']:
        for code in game['codes']:
            key = (game['serial'], code.get('file', ''), code['name'])
            kind = wanted.get(key)
            if kind is None:
                continue
            hit.add(key)
            if not code.get('unsupported'):
                code['unsupported'] = STRUCTURAL_REASONS[kind]
                applied += 1
    return applied, [k for k in wanted if k not in hit]


def load_titles():
    titles = {}
    if TITLES_TSV.exists():
        for line in TITLES_TSV.read_text(encoding='utf-8').splitlines():
            if '\t' in line:
                serial, title = line.split('\t', 1)
                titles[serial.strip().upper()] = title.strip()
    return titles


def main():
    if not PATCHES_DIR.is_dir():
        sys.exit(f'patch directory not found: {PATCHES_DIR}\n'
                 'pass the PS3 folder of a patch database checkout as the first argument')

    db = json.loads(CODES_JSON.read_text(encoding='utf-8'))
    by_serial = {g['serial'].upper(): g for g in db['games']}
    titles = load_titles()

    added_games = added_codes = 0
    topped_up = 0
    skipped = []

    signature = code_signature

    for path in sorted(PATCHES_DIR.glob('*.savepatch')):
        serial = path.stem.upper()
        name, codes, required_blocks = parse_patch(path)

        # A game already in the database gets the extra codes, and its existing
        # codes get the same required steps so they do not write to an encrypted
        # save.
        existing = by_serial.get(serial)
        if existing is not None:
            for code in existing['codes']:
                fold_required(code, required_blocks)
            seen = {signature(c) for c in existing['codes']}
            fresh = []
            for code in codes:
                key = signature(code)
                if key in seen:
                    continue
                seen.add(key)
                fresh.append(code)
            if fresh:
                existing['codes'].extend(fresh)
                added_codes += len(fresh)
                topped_up += 1
            continue

        # A game the other source does not cover becomes a new entry, but never
        # without a real name.
        if not name or name.upper() == serial or SITE.search(name):
            name = titles.get(serial, '')
        if not name or name.upper() == serial:
            skipped.append(serial)
            continue
        game = {
            'serial': serial,
            'title_id': serial,
            'name': name,
            'codes': codes,
        }
        db['games'].append(game)
        by_serial[serial] = game
        added_games += 1
        added_codes += len(codes)

    db['games'].sort(key=lambda g: g['serial'].upper())
    db['source'] = 'merged community sets'
    db['total_games'] = len(db['games'])
    db['total_codes'] = sum(len(g['codes']) for g in db['games'])
    junk, choice = clean_codes(db)
    dropped, renamed = clean_code_names(db)
    filled = fill_hmac_keys(db)
    deduped = dedupe_codes(db)
    structural, structural_missing = apply_structural_overrides(db)

    reasons = {}
    for game in db['games']:
        for code in game['codes']:
            reason = code.get('unsupported')
            if reason:
                reasons[reason] = reasons.get(reason, 0) + 1

    CODES_JSON.write_text(json.dumps(db, indent=2, ensure_ascii=False), encoding='utf-8')

    print(f'new games: {added_games}')
    print(f'games topped up with extra codes: {topped_up}')
    print(f'added codes: {added_codes}')
    print(f'redacted HMAC keys filled in: {filled}')
    print(f'duplicate cheats removed: {deduped}')
    print(f'codes the interpreter refuses, flagged unsupported: {structural}')
    if structural_missing:
        print(f'  WARNING: {len(structural_missing)} structural override(s) matched no code:')
        for key in structural_missing[:10]:
            print(f'    {key}')
    print(f'page wording dropped from code bodies: {junk} entries')
    print(f'cheats flagged as needing a chosen value: {choice}')
    print(f'code names cleaned of site addresses: {renamed} (dropped {dropped} note-only entries)')
    print(f'  unsupported codes: {sum(reasons.values())}')
    for reason, n in sorted(reasons.items(), key=lambda kv: -kv[1]):
        print(f'    {n}: {reason}')
    print(f'skipped (no usable name): {len(skipped)}')
    if skipped:
        print('  ' + ', '.join(skipped[:30]))
    print(f'total games now: {db["total_games"]}')
    print(f'total codes now: {db["total_codes"]}')


if __name__ == '__main__':
    main()
