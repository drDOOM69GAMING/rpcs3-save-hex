#!/usr/bin/env python3
"""Bring an already-merged database up to date without re-running the full merge.

`merge_code_db.py` is the importer that builds `assets/codes.json` from the patch
database. It needs that database plus the community code pages. This tool does
the parts of the import that only need the patches, so a database that was built
before a change can be brought forward in place:

* **Cross-file cheats.** A cheat whose checksum is computed from one file but
  stored in another (Prototype 1/2, Dead Space 2/3) is stored as ordered per-file
  `steps`, because the interpreter has to switch buffers mid-run. An older
  database has those cheats flagged with the "only part of a cheat" reason: the
  seed and store halves of the chain were dropped for targeting a different
  file, leaving a code that reads a value nothing sets. They are rebuilt here.
* **Placeholder values.** A coded line whose value holds a placeholder letter
  (`0000002B 000000xx`) needs the user to choose a value, so it is flagged
  instead of being left for the interpreter to ignore.
* **Note-only entries.** An `[INFO: ...]` note is not a cheat.

Usage:
    python3 tools/repair_db.py [PATCHES_DIR] [CODES_JSON]

Defaults:
    PATCHES_DIR = /tmp/opencode/apollo-patches/PS3
    CODES_JSON  = assets/codes.json
"""

import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import merge_code_db as M  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
PATCHES_DIR = Path(sys.argv[1]) if len(sys.argv) > 1 else Path('/tmp/opencode/apollo-patches/PS3')
CODES_JSON = Path(sys.argv[2]) if len(sys.argv) > 2 else ROOT / 'assets' / 'codes.json'

# A note block such as `[INFO: Some patches may not be supported by BSD yet]`.
NOTE_NAME = re.compile(r'^\s*(?:info|note|tip|warning)\b', re.I)

# A coded line's value the user has to fill in, e.g. `0000002B 000000xx`.
PLACEHOLDER_LINE = re.compile(r'^([0-9A-Fa-f]{8})\s+([0-9A-Za-z?*]{8})$')


def has_placeholder(line):
    """True when a coded line's value still holds a letter to fill in.

    The value must be eight characters of the alphabet a listing uses for codes,
    and at least one of them must not be a hex digit, otherwise an ordinary hex
    value would be mistaken for a placeholder.
    """
    m = PLACEHOLDER_LINE.match(line.strip())
    if not m:
        return False
    value = m.group(2)
    return any(c not in '0123456789abcdefABCDEF' for c in value)

# Reasons that describe a save this tool cannot read at all. A placeholder in one
# of those is not what stops the cheat, so it keeps the stronger reason.
HARD_REASONS = (
    'own encryption',
    'game-specific checksum',
    'compressed',
    'Python script',
)


def own_lines(code, required_blocks):
    """The cheat's own lines, with already-folded required steps taken back out.

    The database holds each cheat's writes followed by the same-file required
    blocks that were folded in. Stripping those trailing blocks again gives the
    program `build_program` needs to re-plan the whole chain from scratch.
    """
    lines = list(code.get('lines', []))
    for r in sorted(required_blocks, key=lambda r: len(r['lines']), reverse=True):
        block = list(r['lines'])
        if block and len(lines) >= len(block) and lines[-len(block):] == block:
            lines = lines[: len(lines) - len(block)]
    return lines


def main():
    if not PATCHES_DIR.is_dir():
        sys.exit(f'patch directory not found: {PATCHES_DIR}\n'
                 'pass the PS3 folder of a patch database checkout as the first argument')

    db = json.loads(CODES_JSON.read_text(encoding='utf-8'))
    by_serial = {g['serial'].upper(): g for g in db['games']}

    notes_removed = repaired = failed = placeholders = 0

    for path in sorted(PATCHES_DIR.glob('*.savepatch')):
        game = by_serial.get(path.stem.upper())
        if game is None:
            continue
        _, _codes, required_blocks = M.parse_patch(path)

        kept = []
        for code in game['codes']:
            # A note with nothing to apply is not a cheat.
            if NOTE_NAME.match(code.get('name', '')) and not code.get('lines'):
                notes_removed += 1
                continue
            if code.get('steps'):
                kept.append(code)
                continue
            if code.get('unsupported') != M.UNSUPPORTED_INCOMPLETE:
                kept.append(code)
                continue
            if not required_blocks:
                failed += 1
                kept.append(code)
                continue

            lines = own_lines(code, required_blocks)
            steps, needs_python = M.build_program(code['file'], lines, required_blocks)
            targets = {M.base_name(t) for t, _ in steps if t}
            reason = M.classify(M.flat_lines(steps), needs_python)

            if len(targets) > 1:
                code['steps'] = [{'file': t, 'lines': ls} for t, ls in steps if ls]
                code['lines'] = lines
            else:
                code['lines'] = M.flat_lines(steps)
                code['steps'] = []

            if reason:
                code['unsupported'] = reason
                failed += 1
            else:
                code.pop('unsupported', None)
                repaired += 1
            kept.append(code)
        game['codes'] = kept

    # Older builds of the importer phrased the value prompt differently. The tool
    # accepts both, but a database should carry one wording.
    OLD_CHOICE = (
        'This cheat needs you to choose an option that this tool cannot prompt '
        'for, so it is listed here but cannot be applied.'
    )
    reworded = 0
    for game in db['games']:
        for code in game['codes']:
            if code.get('unsupported') == OLD_CHOICE:
                code['unsupported'] = M.UNSUPPORTED_CHOICE
                reworded += 1

    # A reason that was true when the database was built may no longer be: a
    # checksum the engine has since learned, or a split listing that now folds.
    # Re-classify and clear only those two reasons, never a stronger one.
    unflagged = 0
    for game in db['games']:
        for code in game['codes']:
            reason = code.get('unsupported')
            if reason not in (M.UNSUPPORTED_HASH, M.UNSUPPORTED_INCOMPLETE):
                continue
            program = code.get('lines') or []
            for step in code.get('steps', []):
                program = program + step.get('lines', [])
            if M.classify(program, False) is None:
                code.pop('unsupported', None)
                unflagged += 1

    # A coded line whose value is still a placeholder needs the user to choose one.
    # The interpreter would ignore the line, so the cheat is flagged instead.
    # This runs after the re-classification above, because a code held back for
    # an unknown checksum can still turn out to need a chosen value.
    for game in db['games']:
        for code in game['codes']:
            reason = code.get('unsupported') or ''
            if any(h in reason for h in HARD_REASONS):
                continue
            body = [l for l in code.get('lines', [])]
            for step in code.get('steps', []):
                body.extend(step.get('lines', []))
            if any(has_placeholder(l) for l in body):
                code['unsupported'] = M.UNSUPPORTED_CHOICE
                placeholders += 1

    # Codes the interpreter refuses outright are flagged with a plain reason, so
    # the UI explains them instead of failing at apply time.
    structural, structural_missing = M.apply_structural_overrides(db)
    if structural_missing:
        print(f'  warning: {len(structural_missing)} structural override(s) matched no code')

    db['total_codes'] = sum(len(g['codes']) for g in db['games'])
    CODES_JSON.write_text(json.dumps(db, indent=2, ensure_ascii=False), encoding='utf-8')

    print(f'note-only entries removed: {notes_removed}')
    print(f'cross-file cheats repaired: {repaired}')
    print(f'still not applicable: {failed}')
    print(f'placeholder codes flagged: {placeholders}')
    print(f'codes re-enabled by a newly supported checksum: {unflagged}')
    print(f'value-prompt wording refreshed: {reworded}')
    print(f'total codes now: {db["total_codes"]}')


if __name__ == '__main__':
    main()