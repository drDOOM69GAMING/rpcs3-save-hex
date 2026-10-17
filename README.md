
<img width="2034" height="1400" alt="Screenshot_20261017_073901" src="https://github.com/user-attachments/assets/e9ba4fc6-40e8-423f-9663-a7eaf789637d" />

<img width="2036" height="1406" alt="Screenshot_20261017_134034" src="https://github.com/user-attachments/assets/6c2d738e-dfa5-493f-a7e1-790b7f16b180" />

# RPCS3 Save Editor

A small desktop app that drops pre-made PS3 save codes into your RPCS3 save
data, with a command-line tool for the times you would rather script it.

The codes and the game titles are baked into the binary. Nothing downloads,
nothing phones home, and once you point it at your save folder it just works.

To be clear about one thing up front: this app does not write new cheats. It
takes codes that already exist and puts them into your save.

## How the codes work

The bundled database was put together from a couple of community code sets.
Where the same game turned up in both, the codes were merged and exact
duplicates dropped, so you get one list per game instead of the same cheat
listed twice.

Plenty of those codes only work if a few other steps happen around them. The
save gets decrypted, the code goes in, the checksums get fixed, the save gets
encrypted again. All of that is folded into the code itself, so you pick one
entry and the app handles the rest in the right order.

## Keeping your saves safe

Applying a code always follows the same three steps, in the app and on the
command line:

1. **Dry run.** The code is tried against a copy of the save, and you are shown
   the exact bytes that would change, as `offset: before -> after`.
2. **Confirm.** Nothing touches the real file until you press **Apply changes**
   in the app, or pass `--yes` on the command line.
3. **Back up.** The first time a file is about to be written, a clean copy is
   saved next to it as `<filename>.bak`. A `.bak` that already exists is never
   overwritten, so applying more codes later can never stomp on your original.

If you would rather not keep backups, there is a **Back up before writing**
checkbox in the app, in the top bar and again in the confirm window. With it
off, applying skips the confirmation step, writes straight away, and pops up a
small **Applied** note with a chime so you still get told what happened. The
command line has `--no-backup` for the same thing.

## Codes that span several files

Some games keep their checksum in a different file from the data. Prototype and
Dead Space are the obvious ones: the cheat edits `USERDATA.BIN`, the checksum
is seeded from `ICON0.PNG`, and the result is stored in `CRC.BIN`. Apply any one
of those bytes on its own and the game throws the save away.

Those codes run as one unit. The interpreter holds every file the code touches
and moves between them, carrying its variables with it, and the app treats the
whole save folder as one change: every file it read is shown, every file it
touched is backed up, and the write only happens once all of them are ready.

## Codes you fill in yourself

Some listings leave a value blank, like `28000170 XXXXXXXX` or
`20000270 xxxx00vv`. Those get a **Value (hex)** box under the code. Type the
digits you want and the app shows the exact line it will use before you apply,
so there is no guessing about what `xxxx00vv` turned into.

A few listings never say which file they patch at all. GTA V's vehicle and paint
codes are the big example. Those get a **Save file** dropdown listing the real
files found in the game's folders, and the code is applied to whichever one you
pick.

## Codes it will not touch

Some entries cannot be applied safely, so they are refused instead of being
written into the wrong bytes:

- **Codes that need a step this app has not implemented.** That covers
  game-specific encryption, compressed saves, and patches that run an embedded
  Python script. They stay in the list with a "not supported" note, so you can
  see what was skipped and why.
- **Checksums that are not recognised.** The bundled set does use a few obscure
  ones, and those are refused with "not supported by this tool". Everything
  else is covered: crc32, crc32big, custom crc, adler32, sha1,
  sha224/256/384/512, md5, md5_xor, sha1_xor64, hmac_sha1, mc02, crc64_ecma,
  jenkins_oaat, rockstar, lookup3, the add/wadd/dwadd/qwadd/wsub family, and the
  game-specific checksums used by Final Fantasy X/X-2 HD Remaster, Kingdom
  Hearts, Tales of Zestiria, Tears to Tiara, Samurai Warriors 4 and Castlevania:
  Lords of Shadow.

## Building it

You need a recent stable Rust toolchain.

```sh
cd rpcs3-save-hex
cargo build --release
```

Then run it through cargo, or launch the binary directly:

```sh
cargo run --release
./target/release/rpcs3-save-hex
```

## Using the app

1. Click **Choose Save Folder...** and point it at the folder holding your save
   data. That is the one full of directories named like
   `BCUS98123_NDI_UNCHARTED2_BT_P`, usually
   `.../dev_hdd0/home/<user>/savedata`.
2. Pick a game on the left. Every game is listed as `Name (SERIAL)`, so you
   never have to line a bare serial up against a title in your head. The name is
   looked up in the cheat database first, then the bundled title list, then the
   save's own `PARAM.SFO`.
3. The codes show up on the right in open and close sections that follow the
   grouping stored in the database. When a game keeps several characters or
   several save slots, each slot gets its own section, and that is what lets you
   tell two codes with the same name apart. The search box matches names, groups
   and file names, and opens every section while you type.
4. Every code has its own green **Apply** button. Press it and a window lists
   each save file the code matched and the bytes it would change, or a clear
   reason it cannot be applied. If the code leaves a value blank you get a
   **Value (hex)** box to fill in first, and if it never names a file you get a
   **Save file** dropdown.
5. Codes that come one per save slot also get an **All slots** button. That one
   patches every slot in a single pass, so the right slot gets hit even when you
   do not know which one your save actually uses. The window then shows which
   slots would change, which already hold the value, and which could not find
   their pattern in that slot.
6. Press **Apply with backup**. The **Back up before writing** checkbox turns
   backups off, and the button label changes to match.

The confirm window is also where it tells you the difference between the two
things that look like a no-op. "Nothing to change" means the save already holds
the values the code writes, so that cheat is already in effect. "Not applied"
means the code could not find what it patches in that file, which usually means
that save slot uses a different layout.

There is also **Advanced Hex Editor**, which opens any file read-only as hex, in
case you just want to look.

## Command-line tool

The `apply` binary does the same work without a window, which is handy for
scripting or for checking a whole save folder in one go:

```sh
# List the codes for a serial
apply db BCUS98123

# Name every save folder under a folder (shows where each name came from)
apply names <save-folder>

# Which files would a code target?
apply files <save-folder> BCUS98123 'BCUS98123_NDI_UNCHARTED2_BT_*\USR-DATA'

# Dry-run a code (no writes)
apply apply-code --root <save-folder> --serial BCUS98123 --code 0

# Same, but write and back up
apply apply-code --root <save-folder> --serial BCUS98123 --code 0 --yes

# Patch every save slot at once (one write, one backup)
apply apply-code --root <save-folder> --serial BCUS98123 --code 0 --all-slots --yes

# Fill in a value a code leaves blank
apply apply-code --root <save-folder> --serial BLES01507 --code 169 --value 0F --yes

# Write without keeping a backup
apply apply-code --root <save-folder> --serial BCUS98123 --code 0 --yes --no-backup

# Dry-run every code of every game found under a folder
apply verify <save-folder>

# Run raw lines against a single file
apply preview <file> 'write at 0x0:FF'
apply preview <file> '20000A78 0000001F'
```

## Code formats it understands

- **Hex code lines** of the form `T B XXXXXX` (8, 16 and 32-bit direct writes,
  pointer-relative writes, and the type-4 search-and-fill that a lot of fighting
  games use).
- **Script codes**: `set`, `write at`, `write next`, `search`, `insert`,
  `delete`, `copy`, the bitwise `and`/`or`/`xor`/`not`, `left`/`right`/`mid`
  slices, all the arithmetic, the `DECRYPT` and `ENCRYPT` ciphers below, and
  checksum setters.
- `:FILE` targets from the database, matched as case-insensitive glob patterns
  (`\` or `/`) against the game's save directories.

Save data is read as big-endian. The standard ciphers a code can name (AES,
Blowfish and 3DES, in ECB or CBC mode) are all implemented, so an encrypted save
gets decrypted, patched and re-encrypted in one apply. Compressed containers are
not unpacked, and neither are the handful of games that wrap their saves in
their own encryption, such as the Final Fantasy XIII saves or Silent Hill HD.
Codes that need any of that are refused rather than applied to the wrong bytes.

## Game names

Every save shows up as `Name (SERIAL)`. The name comes from the first of these
that has an answer:

1. the bundled cheat database,
2. the bundled title list (`assets/titles.tsv`, about 4,500 titles),
3. the title inside the save's own `PARAM.SFO`, falling back to the serial.

Because RPCS3 writes the real title into every `PARAM.SFO`, even a game with no
cheat codes gets a proper name.

## What is in here

```
src/
  main.rs            the app (eframe/egui)
  db.rs              bundled database model and loader
  scan.rs            find saves, group by game, resolve names
  sfo.rs             PARAM.SFO reader (game title)
  apply.rs           resolve targets, dry-run plan, backup, commit
  sound.rs           the short chime the app plays on a successful apply
  lib.rs             library root
  bin/apply.rs       the command-line tool
  engine/
    mod.rs           dispatch, shared helpers
    sw.rs            interpreter for hex code lines
    bsd.rs           interpreter for script codes
    hash.rs          checksums and hashes
    crypto.rs        AES, Blowfish and 3DES
    wildcard.rs      glob matcher
assets/codes.json    the bundled code database
assets/titles.tsv    bundled serial to title fallback list
examples/audit.rs         run every code through the interpreter and report what it cannot parse
examples/noop.rs          list codes that plan a zero-byte change against real saves
examples/hashcheck.rs     print the game checksums so they can be diffed against the reference C
tools/parse_code_set.py  parse the bundled community code set
tools/fetch_titles.py    rebuild assets/titles.tsv from the title database
tools/merge_code_db.py   merge the larger patch database into assets/codes.json
tools/repair_db.py       bring an already built database up to date in place
tools/unsupported_structural.json  codes the interpreter refuses, recorded by running it
```

## Where the database comes from

`assets/codes.json` is built by `tools/merge_code_db.py` from two community
sources: the bundled quick code pages and the Apollo save patch database. Codes
that cannot be applied are kept in the list with a reason attached rather than
dropped, so you can still see that a cheat exists for a game even when this
tool will not write it.

Rebuilding from scratch needs both sources. `tools/repair_db.py` is the lighter
path: it takes an existing `assets/codes.json` and the patch database alone and
brings it forward, which is what you want after the importer gains a new
normalisation.

```sh
python3 tools/repair_db.py /path/to/apollo-patches/PS3
```

## Licence and credits

This project is released under the GNU General Public License, version 3. The
full text is in [LICENSE](LICENSE).

It bundles and ports work from other free projects, and those notices have to
travel with it. They are collected in
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
