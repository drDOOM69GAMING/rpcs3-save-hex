//! The end-to-end "apply a quick code to a real save file" workflow.
//!
//! This is shared by the GUI and the headless CLI so both resolve save files,
//! build a dry-run diff, back the file up and write it exactly the same way.
//!
//! Nothing here writes until [`commit`] is called, so a caller can always show
//! the diff and ask for confirmation first.

use crate::engine::{self, Endian};
use crate::engine::wildcard::wildcard_match_icase;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// One byte the cheat changes. `None` marks a byte added at the end of the file
/// (script `insert`) while the reverse marks a byte removed (script `delete`).
#[derive(Debug, Clone)]
pub struct ByteChange {
    pub offset: usize,
    pub before: Option<u8>,
    pub after: Option<u8>,
}

/// How many individual byte changes are kept for display. `changed` still
/// counts them all.
const MAX_SHOWN_CHANGES: usize = 400;

/// Prefix the hex interpreter uses when a `search` matched nothing.
pub const SEARCH_MISS: &str = "search pattern not found";

/// A ready-to-write result: what the file would become and a summary of the
/// differences from what it is now.
#[derive(Debug, Clone)]
pub struct Plan {
    /// The file this plan applies to.
    pub target: PathBuf,
    /// Length of the file as it is on disk now.
    pub before_len: usize,
    /// The bytes that would be written.
    pub new_data: Vec<u8>,
    /// Total number of differing byte positions.
    pub changed: usize,
    /// The first [`MAX_SHOWN_CHANGES`] differences, for display.
    pub changes: Vec<ByteChange>,
    /// Human-readable trace from the interpreter.
    pub log: Vec<String>,
    /// Lines the interpreter could not fully apply.
    pub warnings: Vec<String>,
}

impl Plan {
    pub fn after_len(&self) -> usize {
        self.new_data.len()
    }

    /// True when nothing changed because a hex `search` found no match, i.e. the
    /// code could not locate what it patches. This is what stops such a code
    /// from being reported as "already matches".
    pub fn search_missed(&self) -> bool {
        self.changed == 0
            && self
                .warnings
                .iter()
                .any(|w| w.starts_with(SEARCH_MISS))
    }
}

/// Split a code's save path into its components, accepting `\` and `/` and
/// ignoring empty pieces.
pub fn split_components(s: &str) -> Vec<String> {
    s.split(|c| c == '\\' || c == '/')
        .filter(|p| !p.is_empty())
        .map(|p| p.to_string())
        .collect()
}

/// Does a save directory belong to `serial`? RPCS3 save directories begin with
/// the serial and then continue with `-`, `_` or more characters.
fn dir_belongs_to(dir: &str, serial_up: &str) -> bool {
    dir.to_ascii_uppercase().starts_with(serial_up)
}

/// Match a code path against an on-disk path: the pattern is matched against
/// the trailing components, so nested save layouts still resolve.
fn matches_tail(components: &[String], pattern: &[String]) -> bool {
    if pattern.is_empty() || pattern.len() > components.len() {
        return false;
    }
    let start = components.len() - pattern.len();
    pattern
        .iter()
        .enumerate()
        .all(|(i, p)| wildcard_match_icase(&components[start + i], p))
}

/// Find the save files under `root` that a code targets.
///
/// `serial` restricts the search to that game's save directories; `pattern` is
/// the code's `file` value (globs allowed). An empty pattern matches nothing,
/// because such a code does not say which file it modifies.
pub fn resolve_files(root: &Path, serial: &str, pattern: &str) -> Vec<PathBuf> {
    let serial_up = serial.to_ascii_uppercase();
    let pat = split_components(pattern);
    let mut out: Vec<PathBuf> = Vec::new();

    for entry in WalkDir::new(root).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(root) else {
            continue;
        };
        let components: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect();
        if components.is_empty() {
            continue;
        }

        // Keep only files that live in a directory named for this serial, so a
        // shared filename such as "USR-DATA" cannot leak across games.
        if !serial_up.is_empty() && !dir_belongs_to(&components[0], &serial_up) {
            continue;
        }

        if matches_tail(&components, &pat) {
            out.push(entry.path().to_path_buf());
        }
    }

    out.sort();
    out.dedup();
    out
}

/// The save files a cheat with no target file of its own could be applied to.
///
/// Some listings (Grand Theft Auto V's vehicle and paint codes among them) never
/// say which file they patch, so the UI offers the real files found under the
/// game's folders and lets the user choose. Save-folder furniture such as the
/// icon and `PARAM.SFO` is left out: it is never what a cheat writes.
pub fn candidate_files(root: &Path, serial: &str) -> Vec<PathBuf> {
    const NOT_SAVES: &[&str] = &["ICON0.PNG", "ICON1.PNG", "PARAM.SFO"];
    resolve_files(root, serial, "*")
        .into_iter()
        .filter(|p| {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().to_ascii_uppercase())
                .unwrap_or_default();
            !NOT_SAVES.contains(&name.as_str())
                && !name.starts_with("PIC")
                && !name.ends_with(".BAK")
        })
        .collect()
}

/// Detect a placeholder code such as `xxxx0000`.
///
/// A qualifying token is at least three characters of hex digits (or `x`) with
/// at least one `x`, after an optional `0x` prefix. Comment lines are ignored,
/// and ordinary words (`index`, `matrix`) are not hex-shaped so they never
/// match.
pub fn placeholder_token(lines: &[String]) -> Option<String> {
    for line in lines {
        let t = line.trim();
        if t.is_empty() || t.starts_with(';') {
            continue;
        }
        for tok in t.split(|c: char| !c.is_ascii_alphanumeric()) {
            let s = tok
                .strip_prefix("0x")
                .or_else(|| tok.strip_prefix("0X"))
                .unwrap_or(tok);
            if s.len() >= 3
                && (s.contains('x') || s.contains('X'))
                && s.chars().all(|c| c.is_ascii_hexdigit() || c == 'x' || c == 'X')
            {
                return Some(t.to_string());
            }
        }
    }
    None
}

/// Validate a code's lines and return the ones the interpreter should run.
pub fn runnable_lines(lines: &[String]) -> Result<Vec<String>, String> {
    let lines: Vec<String> = lines
        .iter()
        .map(|l| l.trim_end_matches(['\r', '\n']).to_string())
        .collect();

    let has_content = lines.iter().any(|l| {
        let t = l.trim();
        !t.is_empty() && !t.starts_with(';')
    });
    if !has_content {
        return Err("this code has no code lines to apply".to_string());
    }

    if let Some(tok) = placeholder_token(&lines) {
        return Err(format!(
            "this code is a placeholder ({}) and contains no real values",
            tok.trim()
        ));
    }

    Ok(lines)
}

/// Why a database code cannot be applied, if it cannot. Covers codes marked
/// unsupported when the database was built (for example, a patch that needs a
/// decryption step this tool does not perform) as well as the empty/placeholder
/// codes the interpreter would refuse anyway.
pub fn code_error(code: &crate::db::Code) -> Option<String> {
    if let Some(reason) = &code.unsupported {
        return Some(reason.clone());
    }
    if !code.steps.is_empty() {
        // A multi-file cheat is validated as the whole program it runs.
        let all: Vec<String> = code.steps.iter().flat_map(|s| s.lines.clone()).collect();
        return runnable_lines(&all).err();
    }
    runnable_lines(&code.lines).err()
}

// ---------------------------------------------------------------------------
// Cheats whose value the user picks
// ---------------------------------------------------------------------------

/// Characters a listing uses to mean "the user fills this in".
///
/// The listings spell this several ways (`xx`, `XXXX`, `yy`, `vv`, `gh`, `?`),
/// so the rule is simply: a letter that is not a hex digit.
fn is_placeholder(c: char) -> bool {
    !c.is_ascii_hexdigit()
}

/// A coded line's value token, when it still holds a placeholder character.
fn placeholder_token_of(line: &str) -> Option<(&str, &str)> {
    let t = line.trim();
    let (head, tail) = t.split_once(char::is_whitespace)?;
    let value = tail.trim();
    let head_ok = head.len() == 8 && head.chars().all(|c| c.is_ascii_hexdigit());
    let value_ok =
        value.len() == 8 && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '?' || c == '*');
    if head_ok && value_ok && value.chars().any(is_placeholder) {
        Some((head, value))
    } else {
        None
    }
}

/// True when any of `lines` is a coded line with a placeholder value, i.e. the
/// cheat cannot be applied until the user picks a value.
pub fn needs_value(lines: &[String]) -> bool {
    lines.iter().any(|l| placeholder_token_of(l).is_some())
}

/// Fill every placeholder in the coded lines of `lines` with digits from `value`.
///
/// The value is read as hex, taken left to right and wrapped, so a short value
/// repeated fills a longer placeholder. Digits the listing already spells out
/// are left alone. The result is a plain copy of the cheat with real values in
/// place of the letters.
pub fn fill_placeholders(lines: &[String], value: &str) -> Result<Vec<String>, String> {
    let digits: Vec<char> = value
        .trim()
        .trim_start_matches("0x")
        .trim_start_matches("0X")
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect();
    if digits.is_empty() {
        return Err("Enter the value to use as one or more hex digits.".to_string());
    }

    let mut at = 0usize;
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        match placeholder_token_of(line) {
            None => out.push(line.clone()),
            Some((head, token)) => {
                let filled: String = token
                    .chars()
                    .map(|c| {
                        if is_placeholder(c) {
                            let d = digits[at % digits.len()];
                            at += 1;
                            d
                        } else {
                            c
                        }
                    })
                    .collect();
                out.push(format!("{head} {filled}"));
            }
        }
    }
    Ok(out)
}

/// True when a code is held back only because it needs a chosen value.
pub fn is_choice_reason(reason: &str) -> bool {
    reason.contains("choose an option") || reason.contains("choose a value")
}

/// A copy of `code` with the user's value filled in, ready to plan.
///
/// Only a cheat whose reason was "needs a chosen value" is unlocked; anything
/// else keeps its reason, so a value can never get round a real limitation.
pub fn resolve_choice(code: &crate::db::Code, value: &str) -> Result<crate::db::Code, String> {
    let mut out = code.clone();
    if out.steps.is_empty() {
        out.lines = fill_placeholders(&code.lines, value)?;
    } else {
        for step in &mut out.steps {
            step.lines = fill_placeholders(&step.lines, value)?;
        }
    }
    if out.unsupported.as_deref().is_some_and(is_choice_reason) {
        out.unsupported = None;
    }
    Ok(out)
}

/// Every line of a code, however it is stored.
pub fn code_lines(code: &crate::db::Code) -> Vec<String> {
    if code.steps.is_empty() {
        code.lines.clone()
    } else {
        code.steps.iter().flat_map(|s| s.lines.clone()).collect()
    }
}

/// Build a plan for applying `lines` to `path`, without touching the file.
pub fn plan(path: &Path, lines: &[String]) -> Result<Plan, String> {
    let lines = runnable_lines(lines)?;
    let original =
        fs::read(path).map_err(|e| format!("could not read {}: {}", path.display(), e))?;

    let result = engine::apply_code(&original, &lines, Endian::Big)
        .map_err(|e| format!("could not apply the code: {}", e))?;

    let (changed, changes) = diff(&original, &result.data);
    Ok(Plan {
        target: path.to_path_buf(),
        before_len: original.len(),
        new_data: result.data,
        changed,
        changes,
        log: result.log,
        warnings: result.warnings,
    })
}

/// Build a plan for a database code, refusing up front when the code is marked
/// unsupported (rather than reading files just to fail).
pub fn plan_code(path: &Path, code: &crate::db::Code) -> Result<Plan, String> {
    if let Some(reason) = &code.unsupported {
        return Err(reason.clone());
    }
    if !code.steps.is_empty() {
        return Err(format!(
            "this cheat spans {} files and has to be planned from its save folder, \
             not from a single file",
            code.steps.len()
        ));
    }
    plan(path, &code.lines)
}

/// The last component of a save path, which is what a `:NAME` switch and an
/// in-directory match both need.
fn base_name(pattern: &str) -> String {
    pattern
        .rsplit(['\\', '/'])
        .find(|s| !s.is_empty())
        .unwrap_or(pattern)
        .to_ascii_uppercase()
}

/// Find `pattern`'s file inside one save folder, wildcards allowed.
fn resolve_in(dir: &Path, pattern: &str) -> Option<PathBuf> {
    let want = base_name(pattern);
    let mut hits: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .map(|n| wildcard_match_icase(&n.to_string_lossy(), &want))
                .unwrap_or(false)
        })
        .collect();
    hits.sort();
    hits.into_iter().next()
}

/// Plan a database code against a save folder, returning one plan per file it
/// reads.
///
/// A single-file code yields one plan per matching file, as before. A multi-file
/// code (one whose checksum is computed from one file and stored in another) is
/// run once over every file it names, all inside the same save folder, so the
/// value written to the checksum file matches the bytes the cheat produced.
pub fn plan_code_in(
    root: &Path,
    serial: &str,
    code: &crate::db::Code,
) -> Result<Vec<Plan>, String> {
    if let Some(reason) = &code.unsupported {
        return Err(reason.clone());
    }
    runnable_lines(&code.lines)?;

    if code.steps.is_empty() {
        let files = resolve_files(root, serial, &code.file);
        if files.is_empty() {
            return Err(format!(
                "No save file matched this code's target '{}'. Make sure the game's \
                 save folder is under the selected folder.",
                code.file
            ));
        }
        let mut out = Vec::new();
        for path in files {
            out.push(plan(&path, &code.lines)?);
        }
        return Ok(out);
    }

    let mut out = Vec::new();
    for anchor in resolve_files(root, serial, &code.file) {
        let Some(dir) = anchor.parent() else { continue };
        match plan_multi_in(dir, code) {
            Ok(plans) => out.extend(plans),
            Err(e) => {
                if out.is_empty() {
                    return Err(e);
                }
            }
        }
    }
    if out.is_empty() {
        return Err(format!(
            "No save file matched this code's target '{}'. Make sure the game's \
             save folder is under the selected folder.",
            code.file
        ));
    }
    Ok(out)
}

/// Run a multi-file code over one save folder and diff every file it read.
fn plan_multi_in(dir: &Path, code: &crate::db::Code) -> Result<Vec<Plan>, String> {
    let mut names: Vec<String> = Vec::new();
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut originals: Vec<Vec<u8>> = Vec::new();
    let mut step_index: Vec<usize> = Vec::new();

    for step in &code.steps {
        let pattern = if step.file.trim().is_empty() {
            code.file.as_str()
        } else {
            step.file.as_str()
        };
        let path = resolve_in(dir, pattern).ok_or_else(|| {
            format!(
                "This cheat also patches '{}', which is not in the save folder. \
                 The save it belongs to may not be present.",
                base_name(pattern)
            )
        })?;

        let idx = match paths.iter().position(|p| *p == path) {
            Some(i) => i,
            None => {
                let mut name = base_name(&path.to_string_lossy());
                if names.contains(&name) {
                    name = format!("{name}#{}", names.len());
                }
                let data = fs::read(&path)
                    .map_err(|e| format!("could not read {}: {}", path.display(), e))?;
                names.push(name);
                originals.push(data);
                paths.push(path);
                paths.len() - 1
            }
        };
        step_index.push(idx);
    }

    let mut program = Vec::new();
    for (i, step) in code.steps.iter().enumerate() {
        program.push(format!(":{}", names[step_index[i]]));
        program.extend(step.lines.iter().cloned());
    }

    let files: Vec<engine::NamedBuf> = names
        .iter()
        .cloned()
        .zip(originals.iter().cloned())
        .map(|(name, data)| engine::NamedBuf { name, data })
        .collect();
    let start = step_index.first().copied().unwrap_or(0);

    let (files, log, warnings) = engine::apply_files(files, start, &program, Endian::Big)
        .map_err(|e| format!("could not apply the code: {}", e))?;

    Ok(files
        .into_iter()
        .zip(paths.iter())
        .zip(originals.iter())
        .map(|((file, path), original)| {
            let (changed, changes) = diff(original, &file.data);
            Plan {
                target: path.clone(),
                before_len: original.len(),
                new_data: file.data,
                changed,
                changes,
                log: log.clone(),
                warnings: warnings.clone(),
            }
        })
        .collect())
}

/// Build one plan by applying several codes to `path` in order, each code
/// seeing the previous code's output.
///
/// This exists for the "apply to every save slot" shortcuts: patches that keep
/// several save slots inside one file (Resident Evil HD keeps eight in a single
/// `DATA0.DAT`) name one slot each, so planning them independently would let the
/// last overwrite the earlier ones. Running them in sequence against one buffer
/// patches every slot in a single write and a single backup.
pub fn plan_codes(path: &Path, codes: &[crate::db::Code]) -> Result<Plan, String> {
    let original =
        fs::read(path).map_err(|e| format!("could not read {}: {}", path.display(), e))?;

    let mut data = original.clone();
    let mut log = Vec::new();
    let mut warnings = Vec::new();

    for code in codes {
        if let Some(reason) = &code.unsupported {
            return Err(reason.clone());
        }
        let lines = runnable_lines(&code.lines)?;
        let result = engine::apply_code(&data, &lines, Endian::Big)
            .map_err(|e| format!("could not apply the code: {}", e))?;
        data = result.data;
        log.extend(result.log);
        warnings.extend(result.warnings);
    }

    let (changed, changes) = diff(&original, &data);
    Ok(Plan {
        target: path.to_path_buf(),
        before_len: original.len(),
        new_data: data,
        changed,
        changes,
        log,
        warnings,
    })
}

fn diff(before: &[u8], after: &[u8]) -> (usize, Vec<ByteChange>) {
    let mut changed = 0usize;
    let mut changes = Vec::new();
    let n = before.len().max(after.len());
    for i in 0..n {
        let b = before.get(i).copied();
        let a = after.get(i).copied();
        if b != a {
            changed += 1;
            if changes.len() < MAX_SHOWN_CHANGES {
                changes.push(ByteChange {
                    offset: i,
                    before: b,
                    after: a,
                });
            }
        }
    }
    (changed, changes)
}

/// The `.bak` path used for a save file, e.g. `USR-DATA.bak`.
pub fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".bak");
    path.with_file_name(name)
}

/// Write a plan to disk, backing up the original first.
///
/// The backup is only created if it does not already exist, so re-applying a
/// second code cannot overwrite the pristine copy. Returns the backup path.
pub fn commit(plan: &Plan, backup: bool) -> Result<Option<PathBuf>, String> {
    let mut made_backup = None;
    if backup {
        let bak = backup_path(&plan.target);
        if !bak.exists() {
            fs::copy(&plan.target, &bak)
                .map_err(|e| format!("could not create backup {}: {}", bak.display(), e))?;
        }
        made_backup = Some(bak);
    }

    fs::write(&plan.target, &plan.new_data)
        .map_err(|e| format!("could not write {}: {}", plan.target.display(), e))?;
    Ok(made_backup)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_split_on_both_separators() {
        assert_eq!(
            split_components("BCUS98123_A\\USR-DATA"),
            vec!["BCUS98123_A", "USR-DATA"]
        );
        assert_eq!(split_components("a/b/c"), vec!["a", "b", "c"]);
        assert!(split_components("").is_empty());
    }

    #[test]
    fn tail_matching() {
        let comps: Vec<String> = ["BCUS98123_X", "~extracted", "0000.dat"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let pat: Vec<String> = ["*", "~extracted", "0000.dat"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(matches_tail(&comps, &pat));
        let file_pat = vec!["0000.dat".to_string()];
        assert!(matches_tail(&comps, &file_pat));
        assert!(!matches_tail(&comps, &["nope.dat".to_string()]));
    }

    #[test]
    fn placeholder_detection() {
        assert!(placeholder_token(&["20000A78 xxxx0000".to_string()]).is_some());
        assert!(placeholder_token(&["write at 0x10:xxxxxxxx".to_string()]).is_some());
        assert!(placeholder_token(&["write at 0x10:00FF".to_string()]).is_none());
    }

    #[test]
    fn empty_code_is_refused() {
        assert!(runnable_lines(&["".to_string(), "; comment".to_string()]).is_err());
    }

    #[test]
    fn unsupported_codes_are_refused_with_a_reason() {
        let db = crate::db::Database::load_embedded().unwrap();
        // Max Payne 3 needs a Python step, which this tool cannot run, so its
        // codes are bundled, listed, and refused with that reason.
        let game = db.find_by_serial("BLUS30557").expect("bundled");
        assert!(!game.codes.is_empty());
        assert!(game.codes.iter().all(|c| c.unsupported.is_some()));
        let err = code_error(&game.codes[0]).expect("refused");
        assert!(err.to_lowercase().contains("python"), "unexpected: {err}");
    }

    #[test]
    fn encrypted_saves_get_the_required_steps_folded_in() {
        let db = crate::db::Database::load_embedded().unwrap();
        // Resident Evil HD Remaster's save is encrypted. The import folds the
        // patch's `(Required)` decrypt / SHA1 / encrypt steps into every code so
        // each one can be applied on its own.
        let game = db.find_by_serial("NPUB31552").expect("bundled");
        let code = game
            .codes
            .iter()
            .find(|c| !c.lines.is_empty())
            .expect("a code");
        assert!(code.unsupported.is_none(), "should be applicable");
        assert_eq!(code.lines[0].trim(), "set range:0x000000,eof+1");
        assert!(code.lines[1].to_uppercase().starts_with("DECRYPT BLOWFISH_ECB"));
        let last = code.lines.last().unwrap().to_uppercase();
        assert!(last.starts_with("ENCRYPT BLOWFISH_ECB"), "got {last}");
    }

    #[test]
    fn plain_codes_have_no_marker() {
        let db = crate::db::Database::load_embedded().unwrap();
        let game = db.find_by_serial("NPUB31392").expect("bundled");
        assert!(game.codes.iter().any(|c| c.unsupported.is_none()));
    }

    #[test]
    fn plan_and_commit_round_trip() {
        let dir = std::env::temp_dir().join(format!("rsh_test_{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let file = dir.join("USR-DATA");
        fs::write(&file, [0u8; 16]).unwrap();

        let lines = vec!["write at 4:AABB".to_string()];
        let p = plan(&file, &lines).unwrap();
        assert_eq!(p.before_len, 16);
        assert_eq!(p.changed, 2);

        let bak = commit(&p, true).unwrap().unwrap();
        assert!(bak.exists());
        assert_eq!(fs::read(&file).unwrap()[4..6], [0xAA, 0xBB]);
        assert_eq!(fs::read(&bak).unwrap(), [0u8; 16]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn several_codes_apply_to_one_buffer_in_order() {
        let dir = std::env::temp_dir().join(format!("rsh_multi_{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let file = dir.join("DATA0.DAT");
        fs::write(&file, [0u8; 16]).unwrap();

        // Two save-slot codes writing different offsets. Applying them one after
        // the other on fresh copies of the file would let the second overwrite
        // the first; planning them together must keep both writes.
        let codes = vec![
            crate::db::Code {
                name: "Amos".into(),
                group: "Slot 1".into(),
                lines: vec!["write at 4:AABB".to_string()],
                ..Default::default()
            },
            crate::db::Code {
                name: "Amos".into(),
                group: "Slot 2".into(),
                lines: vec!["write at 8:CCDD".to_string()],
                ..Default::default()
            },
        ];

        let plan = plan_codes(&file, &codes).unwrap();
        assert_eq!(plan.changed, 4);
        assert_eq!(&plan.new_data[4..6], [0xAA, 0xBB]);
        assert_eq!(&plan.new_data[8..10], [0xCC, 0xDD]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_multi_file_code_updates_its_checksum_file() {
        // Dead Space 2's shape: the cheat writes one file, the checksum is
        // seeded from a size field in another file and stored back into it.
        // Every file has to be read in the same save folder and written back
        // together, or the game rejects the save.
        let dir = std::env::temp_dir().join(format!("rsh_multi_cheat_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let save = dir.join("BLTEST001_SAVE");
        fs::create_dir_all(&save).unwrap();

        let mut hed = vec![0u8; 0x100];
        let mut usr = vec![0u8; 0x80];
        usr[0x20] = 0x11;
        hed[0x6C..0x70].copy_from_slice(&(0x81u32).to_be_bytes());
        fs::write(save.join("HED-DATA"), &hed).unwrap();
        fs::write(save.join("USR-DATA"), &usr).unwrap();

        let code = crate::db::Code {
            name: "Test".into(),
            file: "USR-DATA".into(),
            lines: vec!["20000022 AABBCCDD".into()],
            steps: vec![
                crate::db::CodeStep {
                    file: "USR-DATA".into(),
                    lines: vec!["20000022 AABBCCDD".into()],
                },
                crate::db::CodeStep {
                    file: "HED-DATA".into(),
                    lines: vec![
                        "set [usr_size]:read(0x6C,4)".into(),
                        "write at 0x04:0x00000000".into(),
                        "set range:0x0000,eof+1".into(),
                        "set [hash_init]:sdbm".into(),
                    ],
                },
                crate::db::CodeStep {
                    file: "USR-DATA".into(),
                    lines: vec![
                        "set [usr_size]:[usr_size]-1".into(),
                        "set range:0x0000,[usr_size]".into(),
                        "set [hash]:sdbm:[hash_init]".into(),
                    ],
                },
                crate::db::CodeStep {
                    file: "HED-DATA".into(),
                    lines: vec!["write at 0x04:[hash]".into()],
                },
            ],
            ..Default::default()
        };

        let plans = plan_code_in(&dir, "BLTEST001", &code).unwrap();
        assert_eq!(plans.len(), 2, "both files are planned");

        let usr_plan = plans.iter().find(|p| p.target.ends_with("USR-DATA")).unwrap();
        assert_eq!(usr_plan.changed, 4);
        assert_eq!(&usr_plan.new_data[0x22..0x26], [0xAA, 0xBB, 0xCC, 0xDD]);

        let hed_plan = plans.iter().find(|p| p.target.ends_with("HED-DATA")).unwrap();
        // Offset 4 was zeroed and then filled with the new checksum, so all four
        // bytes differ from the pristine save only if the seed mattered.
        assert!(hed_plan.changed >= 4, "checksum must be written back");

        // The stored checksum must match a checksum of the patched data, which is
        // the whole point of the chain.
        let stored = u32::from_be_bytes([
            hed_plan.new_data[4],
            hed_plan.new_data[5],
            hed_plan.new_data[6],
            hed_plan.new_data[7],
        ]);
        let seed = {
            let mut z = hed.clone();
            z[4..8].copy_from_slice(&[0u8; 4]);
            crate::engine::hash::sdbm(&z, 0)
        };
        let want = crate::engine::hash::sdbm(&usr_plan.new_data[..0x80], seed);
        assert_eq!(stored, want);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_chosen_value_fills_a_blank_listing() {
        let lines = vec![
            "000009E3 000000xx".to_string(),
            "42000C28 000000yy".to_string(),
            "set [csum]:0".to_string(),
            "20000270 xxxx00vv".to_string(),
        ];
        assert!(needs_value(&lines));

        let filled = fill_placeholders(&lines, "63").unwrap();
        assert_eq!(filled[0], "000009E3 00000063");
        assert_eq!(filled[1], "42000C28 00000063");
        // A command line is left exactly as written.
        assert_eq!(filled[2], "set [csum]:0");
        // Digits the listing spells out are kept; the blanks take the value in
        // order and wrap when it runs out.
        assert_eq!(filled[3], "20000270 63630063");
        assert!(!needs_value(&filled));

        assert!(fill_placeholders(&lines, "   ").is_err());
        assert!(fill_placeholders(&lines, "0x1F").is_ok());
    }

    #[test]
    fn a_value_only_unlocks_a_value_prompt() {
        let mut code = crate::db::Code {
            name: "Test".into(),
            file: "USR-DATA".into(),
            lines: vec!["000009E3 000000xx".to_string()],
            unsupported: Some(
                "This cheat needs you to choose a value, which you can enter next \
                 to the code before applying it."
                    .to_string(),
            ),
            ..Default::default()
        };
        assert!(code_error(&code).is_some());

        let filled = resolve_choice(&code, "2A").unwrap();
        assert_eq!(filled.lines, vec!["000009E3 0000002A".to_string()]);
        assert!(filled.unsupported.is_none());
        assert!(code_error(&filled).is_none());

        // A cheat held back for a real limitation keeps its reason, so a typed
        // value can never get round it.
        code.unsupported = Some("This save uses the game's own encryption, which this tool cannot decrypt, so the cheat is listed here but cannot be applied.".to_string());
        let still = resolve_choice(&code, "2A").unwrap();
        assert!(still.unsupported.is_some());
        assert!(code_error(&still).is_some());
    }

    #[test]
    fn resolve_requires_a_named_target() {
        let dir = std::env::temp_dir().join(format!("rsh_resolve_{}", std::process::id()));
        let save = dir.join("BCUS98123_NDI_TEST");
        let _ = fs::create_dir_all(&save);
        fs::write(save.join("USR-DATA"), [0u8; 8]).unwrap();
        fs::write(save.join("ICON0.PNG"), [0u8; 8]).unwrap();

        // An empty pattern must never sweep in every file.
        assert!(resolve_files(&dir, "BCUS98123", "").is_empty());

        // A bare filename matches the file, not the icon.
        let files = resolve_files(&dir, "BCUS98123", "USR-DATA");
        assert_eq!(files, vec![save.join("USR-DATA")]);

        // A globbed directory component matches too.
        let files = resolve_files(&dir, "BCUS98123", "BCUS98123_*\\USR-DATA");
        assert_eq!(files, vec![save.join("USR-DATA")]);

        // A different serial must not match this save folder.
        assert!(resolve_files(&dir, "BCUS00000", "USR-DATA").is_empty());

        let _ = fs::remove_dir_all(&dir);
    }
}
